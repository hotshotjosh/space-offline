//! **Dictors and heavy interdictors camp** (BP-CAMP, issue #125, L8 of
//! `handback/r-bot-pvp/report.md`). A doctrine's Interdict role used to be
//! fitted, bought and left idle: the escort never loads the sphere launcher
//! (`weapon_effect` skips it, BP-HOLD) and nothing ran the field generator.
//! This module is the behaviour. It runs inside the fight (`hunt::fight`, so
//! every gang fight that goes through it too) for a bot whose doctrine role
//! is [`Role::Interdict`] (`Mind::role`, set with the fit each docked turn).
//!
//! - **Dictor** (sphere launcher, group 589): docked, [`load_probes`] puts
//!   the hangar's warp disrupt probes (group 548) in the launcher and
//!   [`SPARE_PROBES`] in the hold; in space [`reload_now`] refills an empty
//!   launcher from the hold, and [`launch_now`] launches one probe when a
//!   hostile ship is within [`LAUNCH_REACH`] and no known sphere covers it,
//!   at most once per [`BUBBLE_EVERY`].
//! - **Heavy interdictor** (field generator, effect 3380): [`field_now`]
//!   switches the field on when the prey is inside the field's radius less
//!   [`FIELD_MARGIN`] **and** a friend is on the grid (a lone HIC is a
//!   feed); [`field_off`] switches it off when the prey is out of reach or
//!   the bot goes home (a retreat).
//! - **Focused point** (#18): a bubble does not hold a ship whose type has
//!   `warpBubbleImmune` (1538) set, the attribute the server's
//!   `bubble::immune` reads: ships whose `warpBubbleImmune` is set (supers,
//!   titans, nullified hulls). While such a ship is in the generator's reach
//!   it is the generator's target ([`immune_target`]), whoever the nearest
//!   prey is: the HIC loads the Focused Warp Disruption Script
//!   ([`SCRIPT_NAME`], looked up by name) from its hold
//!   ([`load_script_now`]), aims the generator at the locked ship
//!   ([`point_now`]: an infinite point, `tackle::focus`), and takes the script
//!   out again [`SCRIPT_HOLD`] after the last such ship was in reach
//!   ([`script_out_now`]). A hull the bundle does not mark but whose pilot
//!   warps out of the bot's own running field is learned ([`learn`]). With no
//!   script in the hold it says why and keeps running the field
//!   ([`Mem::note_focus`]).
//! - **Unit** (#18): a mobile warp disruptor is no interdictor's kit. It is
//!   carried by a gang member that is not one ([`unit_duty`]); the dev grant
//!   goes to a bot named for [`CARRIER_STEM`] ([`gm_lines_carrier`]).
//! - **Band**: the bubbles exist in null sec and wormhole space only
//!   ([`crate::hunt::Ground::allows_bubbles`]); the server refuses elsewhere
//!   and the bot never asks. A system the bot knows nothing of counts as no.
//! - **Its own probe**: the launcher's ship sits in its own sphere (**[A]**:
//!   the server holds the launcher, `bubble::inside` exempts only a field's
//!   own ship), so [`Mem::own_probe_alive`] keeps `warp_bubble::decide` from
//!   burning it out of its own bubble while the probe lives.
//!
//! The Scout role has no behaviour here and its seat stays closed (see
//! `doctrine_assign::seat_open`): the gang's scouts already look, report and
//! cloak (`roam_gang`, CLOAK-2), and a camp scout would add a second copy.
//!
//! Every number here is **[A]** policy and none is in a test by value.
//! Falsifiers are at each constant.

use crate::ItemRow;
use crate::brain::{Act, Ball, Memory, Observation, ParkView, Place};
use crate::doctrine::Role;
use crate::pvp_kit::{KitAct, KitModule, Launcher};
use crate::{Conn, Result};
use std::time::{Duration, Instant};

/// Interdiction sphere launchers and warp disrupt probes **[V]**
/// (`bubble.rs`).
pub const GROUP_LAUNCHER: i64 = eve_server::bubble::GROUP_LAUNCHER;
pub const GROUP_PROBE: i64 = eve_server::bubble::GROUP_PROBE;

/// One probe per this long **[A]**: a probe lives 120 s (`explosionDelay`),
/// so a fresh one goes out just before the last expires. Falsified by a
/// gate that is free for seconds between probes (shorten) or a hold of
/// probes spent on a sphere already up (lengthen).
pub const BUBBLE_EVERY: Duration = Duration::from_secs(110);
/// A hostile ship within this many metres of the dictor is worth a probe
/// **[A]** (the probe's own radius is 20 km at the default,
/// `warp_bubble::DEFAULT_RADIUS`). Falsified by probes dropped on ships that
/// leave the sphere at once (shorten).
pub const LAUNCH_REACH: f64 = 20_000.0;
/// The prey must be this far inside the field's radius before the HIC runs
/// it, metres **[A]**: the prey moves and the server measures centre to
/// centre. Falsified by fields switched on and off around a prey at the edge
/// (widen).
pub const FIELD_MARGIN: f64 = 2_000.0;
/// Spare probes kept in the hold for a reload in space **[A]**. Falsified by
/// a dictor out of probes before it can dock.
pub const SPARE_PROBES: i64 = 3;
/// A refused launch (high sec, no skill) puts the launcher off this long
/// **[A]**.
pub const BLOCKED_BACKOFF: Duration = Duration::from_secs(300);
/// A reload is not sent again before this **[A]**.
pub const RELOAD_LOCKOUT: Duration = Duration::from_secs(10);
/// The dock-side work (grant, load) is looked at this often **[A]**.
pub const DOCK_EVERY: Duration = Duration::from_secs(60);
/// Probes an open-mode dictor is given: the launcher's load and the spare.
/// **[A]**: falsified by a dictor that runs dry before it docks.
pub const GRANT_PROBES: i64 = 2 + SPARE_PROBES;
/// A camper warps to its gate and lands this far off it, metres **[A]**: clear
/// of the jump-in point, inside a sphere's radius. Falsified by a camper
/// that lands on the gate (shrink) or whose probe never covers the gate
/// (the sphere is smaller than the standoff: shorten).
pub const GATE_STANDOFF: f64 = 10_000.0;
/// A camper that is not at its gate is sent no oftener than this **[A]**.
pub const POST_EVERY: Duration = Duration::from_secs(20);
/// A dev-spawned dictor waits this long docked for its granted launcher to be
/// fitted and loaded before it undocks without one **[A]**. Falsified by a
/// dictor that undocks with an empty launcher (lengthen) or sits docked for
/// minutes (the bundle names no launcher: shorten).
pub const KIT_HOLD: Duration = Duration::from_secs(120);
/// The names the open-mode grant looks up in the bundle (a name is no
/// extracted value; the ids are resolved at run time).
pub const LAUNCHER_NAME: &str = "Interdiction Sphere Launcher I";
pub const PROBE_NAME: &str = "Warp Disrupt Probe";
/// The heavy interdictor's part, looked up by name in the bundle at run time
/// (its group and effect are the bundle's, `bubble::EFFECT_FIELD`).
pub const FIELD_NAME: &str = "Warp Disruption Field Generator I";
/// The HIC's focused script, looked up by name at run time (#18). Group 908
/// **[V]** (`tackle::GROUP_HIC_SCRIPT`, a private module of eve-server);
/// effect 6849 turns the generator into a disruption point.
pub const SCRIPT_NAME: &str = "Focused Warp Disruption Script";
pub const GROUP_SCRIPT: i64 = 908;
/// The field generator's group **[V]** (`types/typedata.tsv`, the type named
/// [`FIELD_NAME`]). The sale reads it where it has no `Mem` ([`escort::is_pvp_kit`]);
/// the bot's own lookup stays [`field_group_in`]. Re-check at a re-pin.
pub const GROUP_FIELD: i64 = 899;
/// `warpBubbleImmune` **[V]** (`bubble::ATTR_IMMUNE`): a ship whose type has
/// it non-zero is never held by a bubble, so only a point holds it.
pub const ATTR_IMMUNE: i64 = eve_server::bubble::ATTR_IMMUNE;
/// The script stays loaded this long after the last ship no bubble holds was
/// in reach **[A]**, so a super and its escort at like ranges do not make the
/// generator swap script and field. Falsified by a script swapped out and in
/// within a fight (lengthen) or a field never run after the super leaves
/// (shorten).
pub const SCRIPT_HOLD: Duration = Duration::from_secs(60);
/// A ship inside the bot's own running field this long that then leaves the
/// grid with no wreck and no gate near is taken for immune **[A]**
/// ([`learn`]). Falsified by a bubbleable ship marked immune (lengthen).
pub const LEARN_AFTER: Duration = Duration::from_secs(10);
/// A ship last seen within this of a gate's surface jumped, it did not warp
/// **[A]**, metres.
pub const JUMP_RANGE: f64 = 3_000.0;
/// A vanished ship is learned after this many more observations without a
/// wreck near its last spot **[A]** (a kill's wreck reaches the grid a turn
/// or two after the ship leaves it). Falsified by a killed ship learned
/// (lengthen).
pub const LEARN_TURNS: u8 = 2;
/// A dev bot whose name starts with this is the camp's unit carrier (#18):
/// a naming convention between this module and `bot_words.rs` (the
/// `unitcamper` kind's stem), a test hook and no rule. In closed play any gang
/// member that is not an interdictor and holds a unit carries it. Falsified by
/// a carrier that is granted no unit (the two stems differ).
pub const CARRIER_STEM: &str = "Anchors";

/// A mobile warp disruptor (group 361) **[V]**.
pub const GROUP_UNIT: i64 = eve_server::anchor::GROUP_MOBILE_DISRUPTOR;
/// The name the open-mode grant looks up in the bundle (the medium size: it
/// fits a Sabre's hold and its sphere holds a gate; the small one's sphere is
/// too small and the large one does not fit).
pub const UNIT_NAME: &str = "Mobile Medium Warp Disruptor I";
/// The server puts a launched unit this far from the ship (`anchor::DEPLOY_AT`).
pub const DEPLOY_AT: f64 = eve_server::anchor::DEPLOY_AT;
/// The sphere's radius the bot assumes when the bundle's unit type has no
/// `warpScrambleRange` **[A]** ([`Mem::radius`] reads it otherwise).
/// Falsified by a unit launched whose sphere does not hold the gate.
pub const UNIT_RADIUS: f64 = crate::warp_bubble::DEFAULT_RADIUS;
/// The sphere must hold the gate with this much to spare, metres **[A]**.
/// Small on purpose: a standard stargate's warp-in point (radius + 1 km) plus
/// the launch offset must fit a medium unit's sphere. Falsified by a launched
/// unit whose sphere misses ships landing on the gate's warp-in line (raise
/// it) or by a camper that skips every gate (lower it).
pub const UNIT_MARGIN: f64 = 500.0;
/// A camper is "on the warp-in line" when it is within this of its standoff
/// from the gate, metres **[A]**: it warped in at [`GATE_STANDOFF`] along the
/// line the gate's arrivals take. Falsified by a unit that lands off the
/// line the foes warp in on (aim it, not the camper).
pub const LINE_SLACK: f64 = 3_000.0;
/// Anchoring is taken as done this long after the call **[A]** when the
/// bundle's unit type has no `anchoringDelay` ([`Mem::anchor_wait`] reads it
/// otherwise). Falsified by a lift refused as "already changing".
pub const ANCHOR_WAIT: Duration = Duration::from_secs(60);
/// Lifting is taken as done this long after the call **[A]**, then the scoop
/// is tried; a refusal tries again after [`UNIT_BACKOFF`].
pub const LIFT_WAIT: Duration = Duration::from_secs(60);
/// A refused unit call rests the unit's work this long **[A]**.
pub const UNIT_BACKOFF: Duration = Duration::from_secs(30);
/// A hostile ship inside this of the camper makes a scoop unsafe, metres
/// **[A]**.
pub const SAFE_RANGE: f64 = 50_000.0;
/// The camper must be within this of the unit to lift or scoop it, metres
/// **[A]**: well inside the server's reach (`anchor::REACH`, 5 km plus slack).
pub const WORK_RANGE: f64 = 3_000.0;
/// A unit not seen for this long while the camper stands by it is gone **[A]**
/// (a lift or an anchor reaches the client as a removal and a re-add, so a
/// ball can be missing for a pass; a lifting unit gets its lift wait on top).
/// Falsified by a unit forgotten while still there (lengthen) or a shot unit
/// remembered for minutes (shorten).
pub const UNSEEN_GRACE: Duration = Duration::from_secs(30);
/// A unit not seen as a sphere within this of its spot is gone (shot), metres
/// **[A]**.
pub const SPOT_SLACK: f64 = 3_000.0;

/// What the dictor remembers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mem {
    /// When the last probe went out.
    pub launched_at: Option<Instant>,
    /// The launcher is not tried before this (a refusal).
    pub blocked_until: Option<Instant>,
    /// Not before this, the next reload.
    pub reload_until: Option<Instant>,
    /// Not before this, the next dock-side look ([`DOCK_EVERY`]).
    pub dock_next: Option<Instant>,
    /// The open-mode grant was made this session.
    pub granted: bool,
    /// The gate the camper last warped to, and the system it was in.
    pub posted: Option<(i64, i64)>,
    /// Not before this, the next warp to the gate ([`POST_EVERY`]).
    pub post_next: Option<Instant>,
    /// When the dock-side wait for the launcher began ([`KIT_HOLD`]).
    pub kit_wait_from: Option<Instant>,
    /// NW-11b: the mobile warp disruptor this camper launched, if one is out.
    pub unit: Option<Unit>,
    /// A refused launch, anchor, lift or scoop rests the unit's work until
    /// then ([`UNIT_BACKOFF`]).
    pub unit_blocked_until: Option<Instant>,
    /// The unit type's `warpScrambleRange`, metres, read from the bundle by
    /// the driver ([`unit_specs_in`]); `None` until read or when absent.
    pub unit_radius: Option<f64>,
    /// The unit type's `anchoringDelay`, read the same way.
    pub unit_delay: Option<Duration>,
    /// The "gate too big for the unit" line was logged for this camper.
    pub big_gate_logged: bool,
    /// The last unit state logged ([`note_unit`]); a line only on a change.
    pub unit_note: Option<&'static str>,
    /// Since when the unit's sphere has not been seen at its spot while the
    /// camper stood by it ([`UNSEEN_GRACE`]).
    pub unit_unseen: Option<Instant>,
    /// The field generator's group, read from the bundle by [`FIELD_NAME`]
    /// (the driver sets it when it has the content); `None` until then.
    pub field_group: Option<i64>,
    /// The bundle was asked for the field group (found or not), so a missing
    /// one is not looked up again every docked turn.
    pub field_group_looked: bool,
    /// The field generator is not tried before this (it was refused as
    /// offline, SOAK-S4).
    pub field_blocked_until: Option<Instant>,
    /// A stop was sent; not another before this ([`FIELD_OFF_EVERY`]): the
    /// field runs to its cycle end and the server's push says when.
    pub field_off_until: Option<Instant>,
    /// #18: the focused script's type, read from the bundle by
    /// [`SCRIPT_NAME`]; `None` until read or when the bundle names none.
    pub script_type: Option<i64>,
    /// #18: the ship types a bubble does not hold ([`ATTR_IMMUNE`] set),
    /// read from the bundle by the driver ([`immune_types_in`]).
    pub immune: std::collections::BTreeSet<i64>,
    /// #18: probes launched this session (the driver counts them).
    pub launches: u32,
    /// #18: the last launch and focus states logged, and a counter that makes
    /// each change its own `[camp]` line ([`Mem::note`]).
    pub launch_note: Option<&'static str>,
    /// The launch count a `go` line was logged at.
    pub launch_go: Option<u32>,
    pub focus_note: Option<&'static str>,
    pub note_seq: i64,
    /// #18: when a ship no bubble holds was last in the generator's reach
    /// ([`SCRIPT_HOLD`]).
    pub immune_at: Option<Instant>,
    /// #18: hostile pilots (by character) inside the bot's own running field,
    /// since when and where last seen ([`learn`]).
    pub inside: std::collections::BTreeMap<i64, (Instant, [f64; 3])>,
    /// #18: characters learned to be immune for this camp ([`learn`]): the
    /// server reads `warpBubbleImmune` off the dogma sheet (a nullifier fit
    /// counts), the bot only off the hull type.
    pub learned: std::collections::BTreeSet<i64>,
    /// #18: ships that left the grid out of the running field and wait
    /// [`LEARN_TURNS`] for a wreck ([`learn`]).
    pub vanished: Vec<Vanished>,
    /// The docked setup tried the class's guns (#18, [`gunless_hic`]).
    pub gun_fit_tried: bool,
    /// The HIC's standoff once it knew the gate's radius ([`hic_standoff`]),
    /// for a later warp from a grid the gate is not on.
    pub hic_standoff: Option<f64>,
    /// The last standoff decision logged ([`trace_standoff`]).
    pub standoff_note: Option<String>,
    /// The last order the gang gave was the camp ([`order_turn`]).
    pub was_camp: bool,
}

/// A hostile that left the grid out of the bot's own running field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vanished {
    pub char_id: i64,
    pub at: [f64; 3],
    /// Observations still to wait for a wreck.
    pub turns: u8,
}

/// How long between two stops of a running field generator.
pub const FIELD_OFF_EVERY: Duration = Duration::from_secs(5);

/// Where a launched mobile warp disruptor is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// In space, free, not yet anchored.
    Launched,
    /// The anchor call was sent at `at`.
    Anchoring,
    /// Lifting (unanchoring) from `at`.
    Lifting,
}

/// A mobile warp disruptor the camper put out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Unit {
    pub id: i64,
    pub stage: Stage,
    /// When the stage began.
    pub at: Instant,
    /// Where the camper stood when it launched (the unit is [`DEPLOY_AT`]
    /// from there).
    pub spot: [f64; 3],
}

impl Unit {
    /// Anchored: the anchor call was sent and [`ANCHOR_WAIT`] has passed.
    pub fn anchored(&self, now: Instant, wait: Duration) -> bool {
        self.stage == Stage::Anchoring && now.saturating_duration_since(self.at) >= wait
    }
}

/// The smallest standoff a camper asks for, metres **[A]**. A warp to a gate
/// lands at `max(asked, gate radius + 1 km)` (`space.rs`, `WARP_STANDOFF`), so
/// a small ask is raised by the server anyway. Falsified by a camper that
/// never reaches its gate (raise it).
pub const MIN_STANDOFF: f64 = 5_000.0;
/// Room kept between the standoff and the sphere's edge, metres **[A]**: the
/// warp lands with some scatter. Falsified by a launch whose sphere misses the
/// gate by a few hundred metres (raise it).
pub const STANDOFF_BUFFER: f64 = 300.0;

/// **The unit type's sphere radius and anchoring delay** from the bundle
/// (soft): `warpScrambleRange` (103) and `anchoringDelay` (556, ms) of the
/// type named [`UNIT_NAME`]; `None` for what the bundle lacks.
pub fn unit_specs_in(content: &eve_server::content::Content) -> (Option<f64>, Option<Duration>) {
    let Some(t) = eve_server::item::find_type_in(content, UNIT_NAME).map(|t| t.type_id) else {
        return (None, None);
    };
    let attr = |a| eve_server::typedata::attribute_in(content, t, a).filter(|v| *v > 0.0);
    (
        attr(crate::warp_bubble::ATTRIBUTE_RADIUS),
        attr(eve_server::anchor::ATTR_DELAY)
            .and_then(|ms| Duration::try_from_secs_f64(ms / 1000.0).ok()),
    )
}

impl Mem {
    /// The unit's sphere radius: the bundle's, else [`UNIT_RADIUS`].
    pub fn radius(&self) -> f64 {
        self.unit_radius.unwrap_or(UNIT_RADIUS)
    }

    /// How long an anchor takes: the bundle's delay, else [`ANCHOR_WAIT`].
    pub fn anchor_wait(&self) -> Duration {
        self.unit_delay.unwrap_or(ANCHOR_WAIT)
    }

    /// How long a lift takes: the bundle's delay, else [`LIFT_WAIT`].
    pub fn lift_wait(&self) -> Duration {
        self.unit_delay.unwrap_or(LIFT_WAIT)
    }

    /// **The distance a camper stands off its gate**: [`GATE_STANDOFF`], or
    /// closer when the unit's sphere would not hold the gate from there with
    /// the launch offset and [`UNIT_MARGIN`] to spare (never under
    /// [`MIN_STANDOFF`]).
    pub fn standoff(&self) -> f64 {
        GATE_STANDOFF
            .min(self.radius() - DEPLOY_AT - UNIT_MARGIN - STANDOFF_BUFFER)
            .max(MIN_STANDOFF)
    }
}

impl Mem {
    /// Whether the bot's own probe is still up (so it is held by its own
    /// sphere, **[A]**).
    pub fn own_probe_alive(&self, now: Instant) -> bool {
        self.launched_at
            .is_some_and(|at| now.saturating_duration_since(at) < BUBBLE_EVERY)
    }

    /// **One `[camp]` line per change of state** (soft): `slot` is the last
    /// state logged of its kind; a new state prints, the same one does not.
    fn note(
        &mut self,
        bot: &str,
        kind: &str,
        slot: fn(&mut Mem) -> &mut Option<&'static str>,
        state: &'static str,
        detail: &str,
        ok: bool,
    ) {
        if *slot(self) == Some(state) {
            return;
        }
        *slot(self) = Some(state);
        self.note_seq += 1;
        crate::counter::log_decision("camp", bot, kind, self.note_seq, detail, ok);
    }

    /// A launch state changed (soft): one `[camp] launch` line.
    pub fn note_launch(&mut self, bot: &str, state: &'static str, detail: &str, ok: bool) {
        self.note(bot, "launch", |m| &mut m.launch_note, state, detail, ok);
    }

    /// A focus state changed (soft): one `[camp] focus` line.
    pub fn note_focus(&mut self, bot: &str, state: &'static str, detail: &str, ok: bool) {
        self.note(bot, "focus", |m| &mut m.focus_note, state, detail, ok);
    }

    /// A launch was refused: the launcher rests.
    pub fn blocked(&mut self, now: Instant) {
        self.blocked_until = Some(now + BLOCKED_BACKOFF);
    }

    /// The field generator was refused as offline: it rests (SOAK-S4).
    pub fn field_blocked(&mut self, now: Instant) {
        self.field_blocked_until = Some(now + BLOCKED_BACKOFF);
    }
}

/// **Hold the undock for the kit** (BP-CAMP): a dev-spawned dictor has its
/// launcher granted, fitted and loaded at the dock, a turn or two after its
/// role is known; undocking before that sent it out with no launcher, so it
/// never launched a bubble. True while the Interdict role has no loaded
/// launcher (a HIC: no field or no script, a unit carrier: no unit) and
/// [`KIT_HOLD`] has not run out; the next dock-side look is brought forward.
pub fn kit_pending(m: &mut Memory, o: &Observation) -> bool {
    let carrier = is_carrier(m);
    if m.policy.dev.is_none() || (m.combat.role != Some(Role::Interdict) && !carrier) {
        return false;
    }
    // #18: an interdictor carries no mobile unit. A dictor is ready with a
    // loaded launcher, a HIC with the generator fitted and its script aboard
    // (the bundle names one), the unit carrier with the unit in the hold.
    // A bundle without the type falls through after `KIT_HOLD`.
    let ready = if carrier {
        o.cargo.iter().any(is_unit)
    } else if flies_hic(m, o) {
        o.kit.field.is_some()
            && (m.camp.script_type.is_none()
                || o.cargo.iter().any(|r| is_script(m, r))
                || o.kit.script.is_some_and(|s| s.loaded.is_some()))
    } else {
        o.kit
            .launcher
            .is_some_and(|l| l.loaded.is_some_and(|n| n > 0))
    };
    if ready {
        return false;
    }
    let from = *m.camp.kit_wait_from.get_or_insert(o.now);
    if o.now.saturating_duration_since(from) >= KIT_HOLD {
        return false;
    }
    m.camp.dock_next = None;
    true
}

/// A mobile warp disruptor among item rows.
pub fn is_unit(row: &ItemRow) -> bool {
    row.group_id == GROUP_UNIT
}

/// The focused script among item rows, by the type [`Mem::script_type`] holds
/// (the bundle's, looked up by [`SCRIPT_NAME`]; group 908 holds the scrambling
/// script too, which this bot does not use).
pub fn is_script(m: &Memory, row: &ItemRow) -> bool {
    row.group_id == GROUP_SCRIPT && m.camp.script_type == Some(row.type_id)
}

/// **The focused script's type in `content`** (looked up by [`SCRIPT_NAME`];
/// no id is hard-coded).
pub fn script_type_in(content: &eve_server::content::Content) -> Option<i64> {
    eve_server::item::find_type_in(content, SCRIPT_NAME).map(|t| t.type_id)
}

/// **The ship types a bubble does not hold** (#18): every type of `content`
/// whose [`ATTR_IMMUNE`] is non-zero, the attribute the server reads
/// (`bubble::immune`). Read at run time, never a list of ids here.
pub fn immune_types_in(content: &eve_server::content::Content) -> std::collections::BTreeSet<i64> {
    eve_server::typedata::installed_in(content)
        .table()
        .types()
        .filter(|t| t.attribute(ATTR_IMMUNE).is_some_and(|v| v != 0.0))
        .map(|t| t.type_id)
        .collect()
}

/// The pilot whose ball is `ball`.
fn pilot_of(o: &Observation, ball: i64) -> Option<&crate::hunt::Pilot> {
    o.hunt.pilots.iter().find(|p| p.ball.id == ball)
}

/// Whether `p` is a ship no bubble holds (pure): its hull type is among
/// [`Mem::immune`] (the bundle's attribute), or its pilot was learned
/// ([`learn`]). **[A]** a hull the bundle does not mark and a fit the bot has
/// not seen warp out of its field (a nullifier) count as bubbleable:
/// falsified by a prey that warps out of the running field again and again
/// with the bot never loading its script.
fn immune_pilot(m: &Memory, p: &crate::hunt::Pilot) -> bool {
    p.type_id.is_some_and(|t| m.camp.immune.contains(&t)) || m.camp.learned.contains(&p.char_id)
}

/// **A prey a bubble does not hold** (pure).
pub fn prey_immune(m: &Memory, o: &Observation, prey: &Ball) -> bool {
    pilot_of(o, prey.id).is_some_and(|p| immune_pilot(m, p))
}

/// **The ship the generator is aimed at** (#18): the nearest hostile that no
/// bubble holds and that is inside the generator's reach, whoever the nearest
/// prey is. Sets [`Mem::immune_at`].
fn immune_target(m: &mut Memory, o: &Observation) -> Option<Ball> {
    if !camping(m, o) {
        return None;
    }
    let f = o.kit.field?;
    let park = o.park.as_ref()?;
    let t = o
        .hunt
        .pilots
        .iter()
        .filter(|p| {
            p.hostile
                && immune_pilot(m, p)
                && dist(park.ego.position, p.ball.position) <= f.range - FIELD_MARGIN
        })
        .min_by(|a, b| {
            dist(park.ego.position, a.ball.position)
                .total_cmp(&dist(park.ego.position, b.ball.position))
        })
        .map(|p| p.ball)?;
    m.camp.immune_at = Some(o.now);
    Some(t)
}

/// **Learn a ship the field does not hold** (soft, #18): a hostile that sat
/// inside the bot's own running, unscripted field for [`LEARN_AFTER`] and then
/// left the grid warped out of it, so its pilot is immune for this camp
/// ([`Mem::learned`]). Only when, on the turn it vanished: the field was
/// running; the pilot list was not empty (an empty list is a resync); no
/// station is within docking range of the spot and no gate within
/// [`JUMP_RANGE`] (it docked or jumped); and, [`LEARN_TURNS`] turns later, no
/// wreck is near the spot (it died).
///
/// **[A]** a ship that cloaks inside the field, or logs off, also leaves the
/// grid with none of those signs and is learned wrongly. Falsified by a
/// bubbleable pilot marked immune; the cost is a script loaded and pointed at
/// him (a point holds any ship), and the marks clear when the camp ends
/// ([`camp_over`]).
pub fn learn(m: &mut Memory, o: &Observation) {
    // Pending decisions first: a wreck near the spot drops the ship, else the
    // wait runs down.
    let mut pending = std::mem::take(&mut m.camp.vanished);
    pending.retain_mut(|v| {
        if o.hunt
            .wrecks
            .iter()
            .any(|w| dist(w.position, v.at) <= crate::hunt::WRECK_NEAR)
        {
            return false;
        }
        v.turns = v.turns.saturating_sub(1);
        if v.turns == 0 {
            m.camp.learned.insert(v.char_id);
            let name = m.name.clone();
            m.camp.note_focus(
                &name,
                "learned",
                &format!(
                    "focus=learned: pilot {} warped out of the field, no bubble holds it",
                    v.char_id
                ),
                true,
            );
            m.camp.focus_note = None;
            return false;
        }
        true
    });
    m.camp.vanished = pending;
    let (Some(f), Some(park)) = (o.kit.field, o.park.as_ref()) else {
        m.camp.inside.clear();
        return;
    };
    if !f.active || script_loaded(o) {
        m.camp.inside.clear();
        return;
    }
    let reach = f.range - FIELD_MARGIN;
    for p in o.hunt.pilots.iter().filter(|p| p.hostile) {
        if dist(park.ego.position, p.ball.position) <= reach {
            let e = m
                .camp
                .inside
                .entry(p.char_id)
                .or_insert((o.now, p.ball.position));
            e.1 = p.ball.position;
        } else {
            m.camp.inside.remove(&p.char_id);
        }
    }
    let gone: Vec<i64> = m
        .camp
        .inside
        .keys()
        .copied()
        .filter(|c| !o.hunt.pilots.iter().any(|p| p.char_id == *c))
        .collect();
    if o.hunt.pilots.is_empty() {
        // A resync, not a departure.
        m.camp.inside.clear();
        return;
    }
    for c in gone {
        let Some((since, at)) = m.camp.inside.remove(&c) else {
            continue;
        };
        let docked = park
            .station
            .is_some_and(|st| dist(st.position, at) - st.radius < crate::DOCK_RANGE);
        let jumped = o
            .warp_bubble
            .gates
            .iter()
            .any(|g| dist(g.position, at) - g.radius <= JUMP_RANGE);
        let died = o
            .hunt
            .wrecks
            .iter()
            .any(|w| dist(w.position, at) <= crate::hunt::WRECK_NEAR);
        if !docked && !jumped && !died && o.now.saturating_duration_since(since) >= LEARN_AFTER {
            m.camp.vanished.push(Vanished {
                char_id: c,
                at,
                turns: LEARN_TURNS,
            });
        }
    }
}

/// **The camp order is over** (#18): what the camp learned about the prey
/// goes ([`Mem::learned`], [`Mem::inside`], the pending ones and the hold of
/// the script), so the next camp starts clean.
pub fn camp_over(m: &mut Memory) {
    m.camp.was_camp = false;
    m.camp.learned.clear();
    m.camp.inside.clear();
    m.camp.vanished.clear();
    m.camp.immune_at = None;
}

/// **The gang's order this turn** (#18): `camp` is whether it is the camp
/// order. What the camp learned goes only on the change from the camp to
/// another order, not on every turn of another order: a HIC in a roaming
/// gang's fight also runs the generator and keeps its hold.
pub fn order_turn(m: &mut Memory, camp: bool) {
    if m.camp.was_camp && !camp {
        camp_over(m);
    }
    m.camp.was_camp = camp;
}

/// **Whether the carrier may take its unit now** (pure, #18): it flies its
/// kind's hull or a combat hull. The grant and the move to the hold wait for the boarding: the
/// unit moved to the rookie ship's hold stayed behind in it (the log of
/// regress-w32b: granted at +0 s, `Board` at +10 s, `unit=none in hold`).
pub fn carrier_may_take_unit(m: &Memory, o: &Observation) -> bool {
    let flying = o.ship.and_then(|s| s.type_id);
    // The hull its kind names (a Stabber is no entry of the combat table),
    // or one of the table.
    flying.is_some_and(|t| {
        m.policy.dev.as_ref().and_then(|d| d.hull) == Some(t)
            || crate::hull_choice::is_combat_hull(t)
    })
}

/// **The hold the carrier's hull has, and the volume of the unit** (#18, from
/// the bundle): `(capacity, volume)`. The unit must fit the hold to be loaded
/// (the Rifter's 140 m3 refused the 195 m3 Medium unit in regress-w32c, and
/// the refusal was a note nobody read).
pub fn hold_for_unit(content: &eve_server::content::Content, hull: i64) -> Option<(f64, f64)> {
    let table = eve_server::typedata::installed_in(content).table();
    let unit = eve_server::item::find_type_in(content, UNIT_NAME)?.type_id;
    Some((table.get(hull)?.capacity, table.get(unit)?.volume))
}

/// Whether this bot is the dev unit carrier ([`CARRIER_STEM`]). The name is
/// the stem with the side's prefix before it and the group's numbers after
/// (`RedAnchors1x1`).
pub fn is_carrier(m: &Memory) -> bool {
    m.policy.dev.is_some() && carrier_name(&m.name)
}

/// A bot name `[Red]{stem}{N}x{M}` (`bot_control::fresh_group`): the optional
/// side prefix, [`CARRIER_STEM`], then a digit. `Anchorsmith` is not one.
pub fn carrier_name(name: &str) -> bool {
    name.strip_prefix("Red")
        .unwrap_or(name)
        .strip_prefix(CARRIER_STEM)
        .and_then(|rest| rest.chars().next())
        .is_some_and(|c| c.is_ascii_digit())
}

/// **How many guns an interdictor's docked setup asks for** (#18): at most
/// its hull's turret slots (a HIC has none or few; `NotEnoughTurretSlots`
/// refused the three it asked for). Other roles, and a hull the doctrine does
/// not list, ask for the class's number.
pub fn guns_wanted(m: &Memory, flying: Option<i64>, class_guns: usize) -> usize {
    if m.combat.role != Some(Role::Interdict) {
        return class_guns;
    }
    match m.policy.pvp.as_ref().and_then(|g| g.turret_slots(flying)) {
        Some(slots) => class_guns.min(usize::try_from(slots).unwrap_or(0)),
        None => class_guns,
    }
}

/// **A HIC that needs no guns to leave the dock** (#18): it flies a heavy
/// interdictor, its field is **fitted**, and the guns of its class are not to
/// be had (none in the hangar, or the gun fit was tried and a fit was
/// refused). The field is its weapon; a missing gun kept it docked for good.
/// A field only in the hangar is not enough ([`field_to_fit`] fits it first).
pub fn gunless_hic(m: &Memory, o: &Observation, gun_in_hangar: bool) -> bool {
    m.combat.role == Some(Role::Interdict)
        && flies_hic(m, o)
        && o.kit.field.is_some()
        && (!gun_in_hangar
            || (m.camp.gun_fit_tried && o.failed(crate::brain::ActKind::FitAuto) > 0))
}

/// **The HIC's field in the hangar, to fit before the undock** (#18): the
/// guns' wait would otherwise keep the dock decision from ever reaching the
/// stock fit. Tried once per item (`hunt.tackle_tried`); a refused fit leaves
/// the HIC docked.
pub fn field_to_fit<'a>(m: &Memory, o: &'a Observation) -> Option<&'a ItemRow> {
    if m.combat.role != Some(Role::Interdict) || !flies_hic(m, o) || o.kit.field.is_some() {
        return None;
    }
    o.hangar
        .iter()
        .filter(|r| is_field(m, r) && !m.hunt.tackle_tried.contains(&r.item_id))
        .min_by_key(|r| r.item_id)
}

std::thread_local! {
    /// Whether the bot of this thread needs the camp's kit, so the sale and the
    /// hold do not take it ([`is_camp_kit`]); the bot sets it each docked
    /// decision ([`needs_kit`]). The sale's helpers read only item rows.
    static KEEP_KIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Set what [`is_camp_kit`] answers on this thread.
pub fn set_keep_kit(keep: bool) {
    KEEP_KIT.with(|k| k.set(keep));
}

/// **Whether the bot needs the camp's kit** (#18): an Interdict role, a HIC
/// hull, or the unit carrier. Any other role sells or refines a looted
/// generator, script or unit as loot, as before.
pub fn needs_kit(m: &Memory, o: &Observation) -> bool {
    m.combat.role == Some(Role::Interdict) || flies_hic(m, o) || is_carrier(m)
}

/// **A row of the camp's kit the bot needs** (#18): a field generator, a
/// focused script or a mobile unit, for a bot that [`needs_kit`].
pub fn is_camp_kit(row: &ItemRow) -> bool {
    KEEP_KIT.with(|k| k.get())
        && (row.group_id == GROUP_FIELD || row.group_id == GROUP_SCRIPT || is_unit(row))
}

/// **Whether the ship flown is an interdictor** (pure): the Interdict role,
/// or a hull of the bubble groups. It never carries or anchors a unit (#18).
pub fn is_interdictor(m: &Memory, o: &Observation) -> bool {
    m.combat.role == Some(Role::Interdict)
        || m.policy
            .pvp
            .as_ref()
            .is_some_and(|g| g.is_bubbler(o.ship.and_then(|s| s.type_id)))
}

/// **Whether this bot may launch the camp's unit** (pure): in space, in a
/// system where bubbles may be made, flying anything but an interdictor.
pub fn unit_duty(m: &Memory, o: &Observation) -> bool {
    let Place::InSpace { system, .. } = o.place else {
        return false;
    };
    !is_interdictor(m, o)
        && m.policy
            .pvp
            .as_ref()
            .is_some_and(|g| g.allows_bubbles(system))
}

/// **The unit's sphere would hold the gate** (pure): the camper's standoff
/// plus the launch offset, inside the sphere with [`UNIT_MARGIN`] to spare.
pub fn unit_covers_gate(ego: [f64; 3], gate: [f64; 3], radius: f64) -> bool {
    dist(ego, gate) + DEPLOY_AT + UNIT_MARGIN < radius
}

/// **A gate whose warp-in point the unit's sphere cannot hold** (pure): a warp
/// lands at `max(standoff, gate radius + 1 km)`, and from there the launch
/// offset and margin must fit inside the sphere.
pub fn gate_too_big(m: &Memory, gate: &Ball) -> bool {
    let landing = m
        .camp
        .standoff()
        .max(gate.radius + eve_server::space_state::WARP_STANDOFF);
    landing + DEPLOY_AT + UNIT_MARGIN >= m.camp.radius()
}

/// **Whether the camper stands on the gate's warp-in line** (pure): at its
/// standoff from the gate, give or take [`LINE_SLACK`].
pub fn on_line(ego: [f64; 3], gate: [f64; 3], standoff: f64) -> bool {
    (dist(ego, gate) - standoff).abs() <= LINE_SLACK
}

/// **Launch or anchor the camp's unit** (pure): the camp is on and the
/// carrier stands at its gate.
/// - Never on an interdictor hull ([`unit_duty`]): a dictor bubbles with its
///   probes, a HIC with its field.
/// - Nothing is out, the hold has a unit, the camper is on the warp-in line,
///   its sphere would hold the gate and no sphere holds it yet (a unit or a
///   probe already there): launch.
/// - A unit is out and free: anchor it.
pub fn unit_deploy(m: &Memory, o: &Observation) -> Option<Act> {
    if !unit_duty(m, o) || m.homing.is_some() {
        return None;
    }
    let park = o.park.as_ref()?;
    if park.ego_warping || m.camp.unit_blocked_until.is_some_and(|t| o.now < t) {
        return None;
    }
    if let Some(u) = m.camp.unit {
        return (u.stage == Stage::Launched).then_some(Act::Kit(KitAct::UnitAnchor(u.id)));
    }
    let Place::InSpace { system, .. } = o.place else {
        return None;
    };
    let item = o
        .cargo
        .iter()
        .filter(|r| is_unit(r))
        .min_by_key(|r| r.item_id)?;
    let (_, gate) = m.camp.posted.filter(|(s, _)| *s == system)?;
    let gate = o.warp_bubble.gates.iter().find(|g| g.id == gate)?;
    // No `park.station` test: stations are global balls, in every park of the
    // system (SOAK-S2), so a camper in its seat's home system always has one
    // and would never launch (the run that showed it: `unit=ready`, no act).
    if !on_line(park.ego.position, gate.position, m.camp.standoff())
        || !unit_covers_gate(park.ego.position, gate.position, m.camp.radius())
        || o.warp_bubble
            .zones
            .iter()
            .any(|(c, r)| dist(*c, gate.position) < *r)
    {
        return None;
    }
    Some(Act::Kit(KitAct::UnitLaunch(item.item_id)))
}

/// **Why the camp's unit is or is not going out** (pure): one short word for
/// the decision log.
pub fn unit_state(m: &Memory, o: &Observation) -> &'static str {
    if let Some(u) = m.camp.unit {
        return match u.stage {
            Stage::Launched => "launched",
            Stage::Anchoring => "anchoring",
            Stage::Lifting => "lifting",
        };
    }
    if is_interdictor(m, o) {
        return "interdictor carries none";
    }
    if !o.cargo.iter().any(is_unit) {
        return "none in hold";
    }
    let Place::InSpace { system, .. } = o.place else {
        return "not in space";
    };
    if !unit_duty(m, o) {
        return "not camping";
    }
    if m.homing.is_some() {
        return "going home";
    }
    let Some(park) = o.park.as_ref() else {
        return "no park";
    };
    if park.ego_warping {
        return "warping";
    }
    if m.camp.unit_blocked_until.is_some_and(|t| o.now < t) {
        return "resting after a refusal";
    }
    let Some((_, gate)) = m.camp.posted.filter(|(s, _)| *s == system) else {
        return "no gate taken";
    };
    let Some(gate) = o.warp_bubble.gates.iter().find(|g| g.id == gate) else {
        return "gate not on grid";
    };
    if gate_too_big(m, gate) {
        "gate too big"
    } else if !on_line(park.ego.position, gate.position, m.camp.standoff())
        || !unit_covers_gate(park.ego.position, gate.position, m.camp.radius())
    {
        "off line"
    } else if o
        .warp_bubble
        .zones
        .iter()
        .any(|(c, r)| dist(*c, gate.position) < *r)
    {
        "gate already held"
    } else {
        "ready"
    }
}

/// **Log the unit's state when it changes** (soft): a `[camp] ... unit=`
/// decision line once per change.
pub fn note_unit(m: &mut Memory, o: &Observation) {
    let state = unit_state(m, o);
    if m.camp.unit_note == Some(state) {
        return;
    }
    m.camp.unit_note = Some(state);
    let gate = m.camp.posted.map_or(0, |(_, g)| g);
    crate::counter::log_decision(
        "camp",
        &m.name,
        "unit",
        gate,
        &format!("unit={state}"),
        matches!(state, "ready" | "launched" | "anchoring" | "lifting"),
    );
}

/// **Take the unit back** (pure but for forgetting a lost unit): the camp is
/// over (`camp_over`), nothing hostile is near, and the unit is out.
/// - Free (never anchored): scoop it.
/// - Anchored: fly to it, lift it, and scoop it [`LIFT_WAIT`] later.
///
/// A unit not seen as a sphere at its spot, with the bot on that grid, is
/// gone and forgotten.
pub fn unit_retrieve(m: &mut Memory, o: &Observation, camp_over: bool) -> Option<Act> {
    let u = m.camp.unit?;
    let park = o.park.as_ref()?;
    let near = dist(park.ego.position, u.spot) <= WORK_RANGE + SPOT_SLACK;
    if near && !park.ego_warping {
        let seen = o
            .warp_bubble
            .zones
            .iter()
            .any(|(c, _)| dist(*c, u.spot) <= SPOT_SLACK);
        if seen {
            m.camp.unit_unseen = None;
        } else {
            let since = *m.camp.unit_unseen.get_or_insert(o.now);
            let grace = UNSEEN_GRACE
                + if u.stage == Stage::Lifting {
                    m.camp.lift_wait()
                } else {
                    Duration::ZERO
                };
            if o.now.saturating_duration_since(since) >= grace {
                crate::counter::log_decision(
                    "camp",
                    &m.name,
                    "unit",
                    u.id,
                    &format!("unit={:?} forgotten: not seen at its spot", u.stage),
                    false,
                );
                m.camp.unit = None;
                m.camp.unit_unseen = None;
                return None;
            }
        }
    }
    if !camp_over
        || park.ego_warping
        || m.camp.unit_blocked_until.is_some_and(|t| o.now < t)
        || o.hunt
            .pilots
            .iter()
            .any(|p| p.hostile && dist(park.ego.position, p.ball.position) <= SAFE_RANGE)
    {
        return None;
    }
    if !near || dist(park.ego.position, u.spot) > WORK_RANGE {
        return Some(Act::WarpToItem {
            id: u.id,
            range: DEPLOY_AT,
        });
    }
    match u.stage {
        Stage::Launched => Some(Act::Kit(KitAct::UnitScoop(u.id))),
        Stage::Anchoring if u.anchored(o.now, m.camp.anchor_wait()) => {
            Some(Act::Kit(KitAct::UnitLift(u.id)))
        }
        // Still settling: stay on the unit (a rally or a patrol would carry
        // the camper off before the scoop).
        Stage::Anchoring => Some(Act::Wait(crate::brain::SHORT_WAIT)),
        Stage::Lifting if o.now.saturating_duration_since(u.at) >= m.camp.lift_wait() => {
            Some(Act::Kit(KitAct::UnitScoop(u.id)))
        }
        Stage::Lifting => Some(Act::Wait(crate::brain::SHORT_WAIT)),
    }
}

/// **Put a granted unit in the hold, docked** (soft): the hangar's mobile
/// warp disruptor goes to the cargo when the hold has none.
pub fn load_unit(conn: &mut Conn) -> Result<String> {
    if conn.cargo()?.iter().any(is_unit) {
        return Ok(String::new());
    }
    let Some(row) = conn
        .hangar_items()?
        .into_iter()
        .filter(is_unit)
        .min_by_key(|r| r.item_id)
    else {
        return Ok(String::new());
    };
    match conn.load_cargo(row.item_id, Some(1)) {
        Ok(_) => Ok("camp: a mobile warp disruptor in the hold".to_owned()),
        Err(e @ (crate::Error::Io(_) | crate::Error::Protocol(_))) => Err(e),
        Err(e) => Ok(format!("camp unit: {e}")),
    }
}

/// **The focused script to put in the hold** (pure, #18): the lowest-numbered
/// row of the type `script_type` (the bundle's disruption script by name;
/// group 908 holds the scrambling script too, which would block the right
/// one). `None` when the bundle names no script or none of the rows is it.
pub fn pick_script(rows: &[ItemRow], script_type: Option<i64>) -> Option<&ItemRow> {
    let t = script_type?;
    rows.iter()
        .filter(|r| r.group_id == GROUP_SCRIPT && r.type_id == t)
        .min_by_key(|r| r.item_id)
}

/// **Put a HIC's focused script in the hold, docked** (soft, #18): the
/// hangar's script of type `script_type` goes to the cargo when the hold has
/// none of it; the bot loads it in space when a ship no bubble holds is in
/// reach ([`load_script_now`]).
pub fn load_script(conn: &mut Conn, script_type: Option<i64>) -> Result<String> {
    if pick_script(&conn.cargo()?, script_type).is_some() {
        return Ok(String::new());
    }
    let hangar = conn.hangar_items()?;
    let Some(row) = pick_script(&hangar, script_type) else {
        return Ok(String::new());
    };
    match conn.load_cargo(row.item_id, Some(1)) {
        Ok(_) => Ok("camp: a focused script in the hold".to_owned()),
        Err(e @ (crate::Error::Io(_) | crate::Error::Protocol(_))) => Err(e),
        Err(e) => Ok(format!("camp script: {e}")),
    }
}

/// **The field generator's group in `content`** (looked up by
/// [`FIELD_NAME`]; no id is hard-coded).
pub fn field_group_in(content: &eve_server::content::Content) -> Option<i64> {
    eve_server::item::find_type_in(content, FIELD_NAME).map(|t| t.group_id)
}

/// A warp disruption field generator among item rows, by the group
/// [`Mem::field_group`] holds.
pub fn is_field(m: &Memory, row: &ItemRow) -> bool {
    m.camp.field_group == Some(row.group_id)
}

/// Whether the ship flown is a heavy interdictor (its group, by the doctrine's
/// ship table).
pub fn flies_hic(m: &Memory, o: &Observation) -> bool {
    m.policy
        .pvp
        .as_ref()
        .is_some_and(|g| g.is_hic(o.ship.and_then(|s| s.type_id)))
}

/// A sphere launcher among item rows.
pub fn is_launcher(row: &ItemRow) -> bool {
    row.group_id == GROUP_LAUNCHER
}

/// A warp disrupt probe among item rows.
pub fn is_probe(row: &ItemRow) -> bool {
    row.group_id == GROUP_PROBE
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

/// Whether this bot camps now (pure): a doctrine Interdict role, in space,
/// in a system where bubbles may be made.
pub fn camping(m: &Memory, o: &Observation) -> bool {
    if m.combat.role != Some(Role::Interdict) {
        return false;
    }
    let Place::InSpace { system, .. } = o.place else {
        return false;
    };
    m.policy
        .pvp
        .as_ref()
        .is_some_and(|g| g.allows_bubbles(system))
}

/// The ground a ship landing at a gate may cover beyond the gate's radius,
/// metres **[A]**: a warp in lands at its own ask or the gate's radius plus
/// 1 km, a jump in near the gate. A HIC's field must hold all of it
/// ([`hic_standoff`]). Falsified by an arrival the field misses (lengthen).
pub const LANDING_ZONE: f64 = 4_000.0;

/// **How far from its gate a HIC sits** (pure, #18): close enough that its
/// field, with [`FIELD_MARGIN`], holds the whole landing zone (the gate's
/// radius plus [`LANDING_ZONE`]) from the far side of the HIC; no nearer than
/// the server lands a warp (the gate's radius plus 1 km) and no farther than
/// [`GATE_STANDOFF`]. A field too small for a big gate leaves the HIC at the
/// nearest it can sit, and the zone is not all covered.
pub fn hic_standoff(field_range: f64, gate_radius: f64) -> f64 {
    (field_range - FIELD_MARGIN - gate_radius - LANDING_ZONE)
        .min(GATE_STANDOFF)
        .max(gate_radius + eve_server::space_state::WARP_STANDOFF)
}

/// A HIC counts as in place when it is no farther than this beyond its
/// standoff, metres **[A]**. Falsified by a HIC that follows back and forth
/// (widen) or sits outside the zone's reach (narrow).
pub const HIC_SLACK: f64 = 1_000.0;

/// **Say the standoff decision when it changes** (soft, #18): a `[camp]
/// standoff` line with what the decision reads (the HIC, the gate on the grid,
/// the field's range, the distance), so a run shows which condition kept a HIC
/// from moving in.
fn trace_standoff(
    m: &mut Memory,
    o: &Observation,
    park: &ParkView,
    gate: i64,
    at_gate: bool,
    there: bool,
) {
    let g = o.warp_bubble.gates.iter().find(|g| g.id == gate);
    let field = o.kit.field.map(|f| f.range);
    let line = format!(
        "standoff hic={} gate_on_grid={} at_gate={at_gate} there={there} warping={} \
         field_range={field:?} gate_radius={:?} dist={:?} want={:?} default={}",
        flies_hic(m, o),
        g.is_some(),
        park.ego_warping,
        g.map(|g| g.radius),
        g.map(|g| dist(park.ego.position, g.position).round()),
        field.map(|r| g.map_or(r, |g| hic_standoff(r, g.radius))),
        m.camp.standoff(),
    );
    if m.camp.standoff_note.as_deref() != Some(&line) {
        m.camp.note_seq += 1;
        crate::counter::log_decision("camp", &m.name, "standoff", m.camp.note_seq, &line, true);
        m.camp.standoff_note = Some(line);
    }
}

/// **The standoff a camper asks of `gate`**: a HIC's from its field's radius
/// (read from the module, as the bundle gives it), else [`Mem::standoff`].
pub fn standoff_for(m: &Memory, o: &Observation, gate: &Ball) -> f64 {
    match o.kit.field {
        Some(f) if f.range > 0.0 && flies_hic(m, o) => hic_standoff(f.range, gate.radius),
        _ => m.camp.standoff(),
    }
}

/// **The gate a camper holds in `system`** (pure): the lowest-numbered
/// neighbour's gate on the map, or `None` without a map or a gate.
pub fn gate_of(m: &Memory, system: i64) -> Option<i64> {
    let map = m.policy.travel.as_ref()?;
    let mut next = map.neighbours(system);
    next.sort_unstable();
    next.into_iter()
        .find_map(|n| map.gate(system, n).map(|(g, _)| g))
}

/// **Take the gate** (pure but for the timers): warp to `system`'s gate unless
/// the bot is there already (the gate ball is within [`crate::MIN_WARP_DISTANCE`]
/// of it), not while in warp and not oftener than [`POST_EVERY`]. In a system
/// where no bubble may be made there is nothing to camp.
pub fn post(m: &mut Memory, o: &Observation, park: &ParkView, system: i64) -> Option<Act> {
    if !m
        .policy
        .pvp
        .as_ref()
        .is_some_and(|g| g.allows_bubbles(system))
    {
        return None;
    }
    let Some(gate) = gate_of(m, system) else {
        // #136: a camper in a bubble system with no gate to take.
        crate::counter::log_decision(
            "camp",
            &m.name,
            "holding",
            system,
            &format!("gate=none system={system}"),
            false,
        );
        return None;
    };
    // SOAK-S2: stations are global balls, in every park of the system, so
    // "no station in the park" never holds in the home system. The camper is
    // at its gate when the gate ball is within a warp's minimum of it.
    let at_gate = o
        .warp_bubble
        .gates
        .iter()
        .find(|g| g.id == gate)
        .is_some_and(|g| park.ego.surface_to(g) < crate::MIN_WARP_DISTANCE);
    if at_gate && m.camp.posted != Some((system, gate)) {
        // Already there (flown, or warped before this memory): no warp.
        m.camp.posted = Some((system, gate));
    }
    let there = m.camp.posted == Some((system, gate)) && at_gate;
    trace_standoff(m, o, park, gate, at_gate, there);
    if there && !park.ego_warping {
        // #136: the camper holds its gate (it was posted there and its warp
        // has ended, off the station's grid); one line per gate. [A] a turn
        // between the order and the warp showing could log early; falsified
        // by an `ok=1` line before the bot's warp starts.
        crate::counter::log_decision(
            "camp",
            &m.name,
            "holding",
            gate,
            &format!("gate={gate} system={system}"),
            true,
        );
    }
    // #18: a HIC that landed where its warp put it (the gate was off its grid
    // when it asked, so the ask was the dictor's 10 km) closes in to the
    // standoff its field needs, now that the gate is on the grid.
    if there
        && !park.ego_warping
        && flies_hic(m, o)
        && let Some(g) = o.warp_bubble.gates.iter().find(|g| g.id == gate)
        && let Some(f) = o.kit.field.filter(|f| f.range > 0.0)
    {
        let want = hic_standoff(f.range, g.radius);
        m.camp.hic_standoff = Some(want);
        if dist(park.ego.position, g.position) > want + HIC_SLACK
            && m.camp.post_next.is_none_or(|t| o.now >= t)
        {
            m.camp.post_next = Some(o.now + POST_EVERY);
            return Some(Act::Follow {
                id: gate,
                range: want,
            });
        }
    }
    // NW-11b: at the gate with a unit to launch but off the line (a gang that
    // arrived through the gate sits at its jump-in spot): close in to the
    // standoff. A gate too big for the sphere is said once and left alone.
    if there
        && !park.ego_warping
        && m.camp.unit.is_none()
        && o.cargo.iter().any(is_unit)
        && let Some(g) = o.warp_bubble.gates.iter().find(|g| g.id == gate)
    {
        if gate_too_big(m, g) {
            if !m.camp.big_gate_logged {
                m.camp.big_gate_logged = true;
                crate::counter::log_decision(
                    "camp",
                    &m.name,
                    "holding",
                    gate,
                    &format!(
                        "gate too big for the unit gate={gate} radius={} sphere={}",
                        g.radius,
                        m.camp.radius()
                    ),
                    false,
                );
            }
        } else if (!on_line(park.ego.position, g.position, m.camp.standoff())
            || !unit_covers_gate(park.ego.position, g.position, m.camp.radius()))
            && m.camp.post_next.is_none_or(|t| o.now >= t)
        {
            m.camp.post_next = Some(o.now + POST_EVERY);
            return Some(Act::Follow {
                id: gate,
                range: m.camp.standoff(),
            });
        }
    }
    if there
        || park.ego_warping
        || m.camp.post_next.is_some_and(|t| o.now < t)
        || o.failed(crate::brain::ActKind::WarpToItem) >= crate::brain::GIVE_UP_AFTER
    {
        return None;
    }
    m.camp.posted = Some((system, gate));
    m.camp.post_next = Some(o.now + POST_EVERY);
    let range = o
        .warp_bubble
        .gates
        .iter()
        .find(|g| g.id == gate)
        .map_or_else(
            // Off the grid: the standoff a HIC learned at this gate before.
            || {
                m.camp
                    .hic_standoff
                    .filter(|_| flies_hic(m, o))
                    .unwrap_or_else(|| m.camp.standoff())
            },
            |g| standoff_for(m, o, g),
        );
    Some(Act::WarpToItem { id: gate, range })
}

/// **A hostile worth fighting from the gate** (pure): inside [`SAFE_RANGE`].
/// A camper holds its post; a pilot farther off is no prey yet (drones sent at
/// him would strip the post, and the scoop rule counts him as far anyway).
pub fn prey_in_reach(ego: &Ball, prey: &Ball) -> bool {
    ego.surface_to(prey) <= SAFE_RANGE
}

/// **Hold the spot** (pure): a camper's moves are dropped from a fight's acts
/// (the orbit, the follow, any warp), so it never chases off the gate. Shots,
/// locks, ewar and the camp's own acts stay.
pub fn hold_position(acts: Vec<Act>) -> Vec<Act> {
    acts.into_iter()
        .filter(|a| {
            !matches!(
                a,
                Act::Orbit { .. }
                    | Act::Follow { .. }
                    | Act::WarpToItem { .. }
                    | Act::WarpToMember { .. }
            )
        })
        .collect()
}

/// **Refill an empty launcher from the hold** (pure). Not while a probe is
/// out of it being launched, not before the lockout, not after repeated
/// failures.
pub fn reload_now(m: &mut Memory, o: &Observation) -> Option<Act> {
    let l = o
        .kit
        .launcher
        .filter(|l| l.loaded == Some(0) && !l.active)?;
    if m.combat.role != Some(Role::Interdict)
        || m.camp.reload_until.is_some_and(|t| o.now < t)
        || o.failed(crate::brain::ActKind::LoadAmmo) >= crate::brain::GIVE_UP_AFTER
    {
        return None;
    }
    let ship = o.ship?;
    let probes: Vec<i64> = o
        .cargo
        .iter()
        .filter(|r| is_probe(r))
        .map(|r| r.item_id)
        .collect();
    if probes.is_empty() {
        return None;
    }
    m.camp.reload_until = Some(o.now + RELOAD_LOCKOUT);
    Some(Act::LoadAmmo {
        guns: vec![(l.item_id, l.flag_id)],
        charges: probes,
        location: ship.ship_id,
    })
}

/// **Why a probe goes out or does not** (pure): `Ok(launcher)` or the reason
/// in a few words. `None` is the quiet case (not a camper, or the prey out of
/// reach), which is no decision to log.
fn launch_verdict(
    m: &Memory,
    o: &Observation,
    prey: &Ball,
) -> Option<std::result::Result<Launcher, &'static str>> {
    if !camping(m, o) {
        return None;
    }
    let park = o.park.as_ref()?;
    if dist(park.ego.position, prey.position) > LAUNCH_REACH {
        return None;
    }
    let Some(l) = o.kit.launcher else {
        // A HIC has a field, not a launcher: nothing to say.
        return (!flies_hic(m, o)).then_some(Err("no launcher"));
    };
    Some(if l.loaded.is_none_or(|n| n <= 0) {
        Err("launcher empty")
    } else if l.active {
        Err("launcher busy")
    } else if park.ego_warping {
        Err("warping")
    } else if m.camp.blocked_until.is_some_and(|t| o.now < t) {
        Err("resting after a refusal")
    } else if m
        .camp
        .launched_at
        .is_some_and(|at| o.now.saturating_duration_since(at) < BUBBLE_EVERY)
    {
        Err("own probe still up")
    } else if o
        .warp_bubble
        .zones
        .iter()
        .any(|(c, r)| dist(*c, prey.position) < *r)
    {
        // A sphere that holds the prey already needs no second one.
        Err("prey already held")
    } else {
        Ok(l)
    })
}

/// **Launch a probe** (pure): a camper with a loaded launcher, a hostile
/// ship within [`LAUNCH_REACH`] that no known sphere covers, and none sent in
/// the last [`BUBBLE_EVERY`].
pub fn launch_now(m: &Memory, o: &Observation, prey: &Ball) -> Option<Act> {
    match launch_verdict(m, o, prey)? {
        Ok(l) => Some(Act::Kit(KitAct::Launch(l))),
        Err(_) => None,
    }
}

/// **Say why a probe goes out or not** (soft): a `[camp] launch` line per
/// change, for the decision [`launch_verdict`] gives (#18).
fn note_launch(m: &mut Memory, o: &Observation, prey: &Ball) {
    let Some(verdict) = launch_verdict(m, o, prey) else {
        return;
    };
    let name = m.name.clone();
    let n = m.camp.launches;
    match verdict {
        Ok(_) => {
            // Each probe is its own line: the driver counts launches, and a
            // retry before that count moves is the same line.
            if m.camp.launch_go != Some(n) {
                m.camp.launch_go = Some(n);
                m.camp.launch_note = Some("go");
                m.camp.note_seq += 1;
                crate::counter::log_decision(
                    "camp",
                    &name,
                    "launch",
                    m.camp.note_seq,
                    &format!("launch=probe prey={} probe_no={}", prey.id, n + 1),
                    true,
                );
            }
        }
        Err(why) => m
            .camp
            .note_launch(&name, why, &format!("launch=refused: {why}"), false),
    }
}

/// **The generator, when it may be switched on at `prey`** (pure): a camper
/// with an idle, online field generator, the prey inside its radius less
/// [`FIELD_MARGIN`], a friend on the grid, and the capacitor not held. The
/// shared gate of the bubble ([`field_now`]) and the point ([`point_now`]).
fn field_ready(m: &Memory, o: &Observation, prey: &Ball) -> Option<KitModule> {
    if !camping(m, o) || m.homing.is_some() {
        return None;
    }
    let f = o
        .kit
        .field
        .filter(|f| !f.active && f.online && f.range > 0.0)?;
    let park = o.park.as_ref()?;
    if park.ego_warping || o.cap.hold || m.camp.field_blocked_until.is_some_and(|t| o.now < t) {
        return None;
    }
    if dist(park.ego.position, prey.position) > f.range - FIELD_MARGIN {
        return None;
    }
    // Friends committed: someone else is on the grid on our side.
    if !o.hunt.pilots.iter().any(|p| !p.hostile) {
        return None;
    }
    Some(f)
}

/// Whether a focused script is loaded in the generator (the driver reads it).
fn script_loaded(o: &Observation) -> bool {
    o.kit.script.is_some_and(|s| s.loaded.is_some())
}

/// **Run the field** (pure): the bubble, with no script loaded (a scripted
/// generator is refused without a target, `tackle::focus`).
pub fn field_now(m: &Memory, o: &Observation, prey: &Ball) -> Option<Act> {
    if script_loaded(o) {
        return None;
    }
    field_ready(m, o, prey).map(|f| Act::Kit(KitAct::FieldOn(f)))
}

/// **Run the point** (pure, #18): a script is loaded, the generator may run
/// at `prey` ([`field_ready`]) and the prey is locked (the server refuses a
/// scripted activation with no target in the lock list).
pub fn point_now(m: &Memory, o: &Observation, prey: &Ball) -> Option<Act> {
    if !script_loaded(o) || !o.locked.contains(&prey.id) {
        return None;
    }
    field_ready(m, o, prey).map(|f| Act::Kit(KitAct::FieldPoint(f, prey.id)))
}

/// **Stop the generator** (pure): it runs and no stop was sent lately; the
/// load or the unload of a script wants it idle.
fn stop_now(m: &Memory, o: &Observation) -> Option<Act> {
    let f = o.kit.field.filter(|f| f.active)?;
    if m.camp.field_off_until.is_some_and(|t| o.now < t) {
        return None;
    }
    Some(Act::Kit(KitAct::FieldOff(f)))
}

/// **Load the script** (pure, #18): the prey is a ship no bubble holds, no
/// script is in the generator, one is in the hold, the prey is inside the
/// field's reach and the bot may run it. A running generator is stopped
/// first. The load is the one the launcher's reload uses (`Act::LoadAmmo`).
pub fn load_script_now(m: &mut Memory, o: &Observation, prey: &Ball) -> Option<Act> {
    let slot = o.kit.script.filter(|s| s.loaded.is_none())?;
    let f = o.kit.field.filter(|f| f.online && f.range > 0.0)?;
    let ship = o.ship?;
    let park = o.park.as_ref()?;
    if !camping(m, o)
        || m.homing.is_some()
        || park.ego_warping
        || dist(park.ego.position, prey.position) > f.range - FIELD_MARGIN
        || m.camp.reload_until.is_some_and(|t| o.now < t)
        || o.failed(crate::brain::ActKind::LoadAmmo) >= crate::brain::GIVE_UP_AFTER
    {
        return None;
    }
    let scripts: Vec<i64> = o
        .cargo
        .iter()
        .filter(|r| is_script(m, r))
        .map(|r| r.item_id)
        .collect();
    if scripts.is_empty() {
        return None;
    }
    if f.active {
        return stop_now(m, o);
    }
    m.camp.reload_until = Some(o.now + RELOAD_LOCKOUT);
    Some(Act::LoadAmmo {
        guns: vec![(f.item_id, slot.flag_id)],
        charges: scripts,
        location: ship.ship_id,
    })
}

/// **Take the script out** (pure, #18): one is in the generator and no ship
/// no bubble holds has been in reach for [`SCRIPT_HOLD`]. A running generator
/// is stopped first. The script goes to the hold.
pub fn script_out_now(m: &mut Memory, o: &Observation) -> Option<Act> {
    let f = o.kit.field.filter(|f| f.online)?;
    if !script_loaded(o)
        || m.camp.reload_until.is_some_and(|t| o.now < t)
        || m.combat.role != Some(Role::Interdict)
        // A ship no bubble holds was in reach lately: keep the script.
        || m.camp
            .immune_at
            .is_some_and(|t| o.now.saturating_duration_since(t) < SCRIPT_HOLD)
    {
        return None;
    }
    if f.active {
        return stop_now(m, o);
    }
    m.camp.reload_until = Some(o.now + RELOAD_LOCKOUT);
    Some(Act::Kit(KitAct::ScriptOut(f)))
}

/// **The generator's acts at `prey`** (pure but for the timers and the
/// log): what the HIC does about the field, the script and the point.
/// - While any hostile no bubble holds is in the generator's reach, it is the
///   target ([`immune_target`]) whoever the nearest prey is: with a script in
///   the hold the script is loaded and the point aimed at the locked ship;
///   without one the reason is said once and the field runs.
/// - Otherwise the field runs at `prey` ([`field_now`]); a script loaded
///   comes out once [`SCRIPT_HOLD`] has passed since the last such ship.
fn generator_acts(m: &mut Memory, o: &Observation, prey: &Ball) -> Vec<Act> {
    if o.kit.field.is_none() || !camping(m, o) {
        return Vec::new();
    }
    let name = m.name.clone();
    let Some(target) = immune_target(m, o) else {
        if script_loaded(o) {
            if let Some(a) = script_out_now(m, o) {
                m.camp.note_focus(
                    &name,
                    "out",
                    "focus=script out: no ship the bubble misses is in reach",
                    true,
                );
                return vec![a];
            }
            // Held (a ship the bubble misses was in reach lately): the
            // script holds any ship, so the generator points the prey.
            return match point_now(m, o, prey) {
                Some(a) => {
                    m.camp.note_focus(
                        &name,
                        "point-held",
                        &format!("focus=point prey={} (script held)", prey.id),
                        true,
                    );
                    vec![a]
                }
                None => Vec::new(),
            };
        }
        return field_now(m, o, prey).into_iter().collect();
    };
    // `hunt::fight` locks only the nearest hostile: the target is locked here
    // when it is not that one.
    let lock = lock_act(o, &target).filter(|_| target.id != prey.id);
    let target = &target;
    if script_loaded(o) {
        if let Some(a) = point_now(m, o, target) {
            m.camp.note_focus(
                &name,
                "point",
                &format!("focus=point prey={}", target.id),
                true,
            );
            return vec![a];
        }
        return lock.into_iter().collect();
    }
    if let Some(a) = load_script_now(m, o, target) {
        m.camp.note_focus(
            &name,
            "load",
            "focus=script loaded for a ship no bubble holds",
            true,
        );
        return std::iter::once(a).chain(lock).collect();
    }
    // No script to load: say why, and keep the field.
    let why = if o.kit.script.is_none() {
        "focus=none: the script slot is unknown, the field runs"
    } else if m.camp.script_type.is_none() {
        "focus=none: the bundle names no focused script, the field runs"
    } else if !o.cargo.iter().any(|r| is_script(m, r)) {
        "focus=none: no script in the hold, the field runs"
    } else if o.failed(crate::brain::ActKind::LoadAmmo) >= crate::brain::GIVE_UP_AFTER {
        "focus=none: the script will not load, the field runs"
    } else {
        // In the hold, resting after a load, or out of reach: wait for it
        // (a field switched on now would only be stopped for the load).
        return lock.into_iter().collect();
    };
    m.camp.note_focus(&name, "none", why, false);
    field_now(m, o, target).into_iter().collect()
}

/// **Lock the generator's target** (pure, #18): in lock range and neither
/// locked nor being locked.
fn lock_act(o: &Observation, target: &Ball) -> Option<Act> {
    let park = o.park.as_ref()?;
    let gap = park.ego.surface_to(target);
    let slots = if o.lock_max == 0 {
        crate::ewar_aim::LOCK_MAX_DEFAULT
    } else {
        o.lock_max
    };
    (!o.locked.contains(&target.id)
        && !o.pending.contains(&target.id)
        && o.locked.len() + o.pending.len() < slots
        && gap <= o.lock_range * crate::brain::RANGE_MARGIN)
        .then_some(Act::Lock(target.id))
}

/// **Stop the field** (pure): it runs, and the prey is out of reach (or
/// there is none, `prey: None`), or the bot goes home, or no friend is left.
pub fn field_off(m: &Memory, o: &Observation, prey: Option<&Ball>) -> Option<Act> {
    let f: KitModule = o.kit.field.filter(|f| f.active)?;
    if m.camp.field_off_until.is_some_and(|t| o.now < t) {
        return None;
    }
    let park = o.park.as_ref()?;
    let out = prey.is_none_or(|p| dist(park.ego.position, p.position) > f.range + FIELD_MARGIN);
    let alone = !o.hunt.pilots.iter().any(|p| !p.hostile);
    (out || m.homing.is_some() || alone).then_some(Act::Kit(KitAct::FieldOff(f)))
}

/// **The field check at the top of every in-space turn** (pure): the roles'
/// own paths (a gang's orders, the hunter's early returns) never reach
/// [`quiet`], so a field left running is stopped here when the nearest
/// hostile pilot is out of reach, or none is, or the bot goes home.
pub fn field_check(m: &Memory, o: &Observation) -> Option<Act> {
    if m.combat.role != Some(Role::Interdict) {
        return None;
    }
    let park = o.park.as_ref()?;
    let near = o
        .hunt
        .pilots
        .iter()
        .filter(|p| p.hostile)
        .min_by(|a, b| {
            dist(park.ego.position, a.ball.position)
                .total_cmp(&dist(park.ego.position, b.ball.position))
        })
        .map(|p| p.ball);
    field_off(m, o, near.as_ref())
}

/// **The camp's acts for one fight turn** (pure but for the timers and the
/// log): reload, then the probe, then the field, the script and the point.
pub fn acts(m: &mut Memory, o: &Observation, prey: &Ball) -> Vec<Act> {
    if m.combat.role != Some(Role::Interdict) {
        return Vec::new();
    }
    if let Some(a) = reload_now(m, o) {
        return vec![a];
    }
    learn(m, o);
    let mut out = Vec::new();
    let stopped = field_off(m, o, Some(prey));
    let stopping = stopped.is_some();
    out.extend(stopped);
    note_launch(m, o, prey);
    out.extend(launch_now(m, o, prey));
    // One stop is enough: no second act on the generator in the same turn.
    if !stopping {
        out.extend(generator_acts(m, o, prey));
    }
    out
}

/// **The camp with nobody to fight** (pure but for the reload timer): the
/// launcher is refilled, a running field is switched off and an idle
/// generator gives its script up once no ship the bubble misses has been in
/// reach for [`SCRIPT_HOLD`] (#18).
pub fn quiet(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if m.combat.role != Some(Role::Interdict) {
        return Vec::new();
    }
    learn(m, o);
    let mut out: Vec<Act> = reload_now(m, o)
        .into_iter()
        .chain(field_off(m, o, None))
        .collect();
    if out.is_empty() {
        let name = m.name.clone();
        if let Some(a) = script_out_now(m, o) {
            m.camp.note_focus(
                &name,
                "gone",
                "focus=script out: no ship the bubble misses is in reach",
                true,
            );
            out.push(a);
        }
    }
    out
}

/// **The GM lines of the open-mode dictor** (open mode only, once a session;
/// the closed economy buys or builds the part): the launcher and the probes,
/// ids looked up by name in the bundle. `/createitem` gives the skills each
/// asks for (`gm::slash`). Empty when the bundle names neither.
///
/// A heavy interdictor (`hic`) gets the field generator and the focused
/// script (#18) and neither launcher nor probes (its hull refuses the
/// launcher). **No interdictor is granted a mobile warp disruptor** (#18):
/// that is the carrier's ([`gm_lines_carrier`]). Dev mode only: the caller is
/// `camp_dock` under `policy.dev`; in closed play a HIC buys or owns its
/// generator like any module.
pub fn gm_lines(content: &eve_server::content::Content, hic: bool) -> Vec<String> {
    let id = |name: &str| eve_server::item::find_type_in(content, name).map(|t| t.type_id);
    let mut out = Vec::new();
    if hic {
        if let Some(f) = id(FIELD_NAME) {
            out.push(format!("/createitem {f}"));
        }
        if let Some(s) = id(SCRIPT_NAME) {
            out.push(format!("/createitem {s}"));
        }
    } else {
        if let Some(l) = id(LAUNCHER_NAME) {
            out.push(format!("/createitem {l}"));
        }
        if let Some(p) = id(PROBE_NAME) {
            out.push(format!("/createitem {p} {GRANT_PROBES}"));
        }
    }
    out
}

/// **The GM line of the open-mode unit carrier** (#18, open mode only, once a
/// session): one mobile warp disruptor, for a gang member that is no
/// interdictor ([`CARRIER_STEM`]). Empty when the bundle names none.
pub fn gm_lines_carrier(content: &eve_server::content::Content) -> Vec<String> {
    eve_server::item::find_type_in(content, UNIT_NAME)
        .map(|t| format!("/createitem {}", t.type_id))
        .into_iter()
        .collect()
}

/// How many more probes the hold wants when it already has `in_hold`.
pub fn spare_wanted(in_hold: i64) -> i64 {
    (SPARE_PROBES - in_hold).max(0)
}

/// **Load the launcher, docked** (the doctrine fit is on): the hangar's
/// probes go into every empty sphere launcher, and up to [`SPARE_PROBES`]
/// more into the hold. The note says what happened; empty when there was
/// nothing to do. Soft but for a dead connection.
pub fn load_probes(conn: &mut Conn) -> Result<String> {
    let (Some(ship), Some(station)) = (conn.session().ship_id(), conn.session().station_id())
    else {
        return Ok(String::new());
    };
    let launchers: Vec<ItemRow> = conn.fitted()?.into_iter().filter(is_launcher).collect();
    if launchers.is_empty() {
        return Ok(String::new());
    }
    let hangar = conn.hangar_items()?;
    let mut probes: Vec<ItemRow> = hangar.into_iter().filter(is_probe).collect();
    probes.sort_by_key(|r| r.item_id);
    let Some(first) = probes.first().copied() else {
        return Ok(String::new());
    };
    let kind = first.type_id;
    let mut note = Vec::new();
    let mut empty = Vec::new();
    for l in &launchers {
        if conn.loaded_rounds(ship, l.flag_id, kind)? == 0 {
            empty.push((l.item_id, l.flag_id));
        }
    }
    if !empty.is_empty() {
        let ids: Vec<i64> = probes
            .iter()
            .filter(|r| r.type_id == kind)
            .map(|r| r.item_id)
            .collect();
        match conn.load_ammo(&empty, &ids, kind, station) {
            Ok(n) => note.push(format!("loaded {n} probes")),
            Err(e @ (crate::Error::Io(_) | crate::Error::Protocol(_))) => return Err(e),
            Err(e) => note.push(format!("probe load: {e}")),
        }
    }
    // What the launcher left goes in the hold, for a reload in space: up to
    // SPARE_PROBES in all, what the hold has already counted.
    let in_hold = match conn.cargo() {
        Ok(rows) => rows
            .iter()
            .filter(|r| is_probe(r))
            .map(|r| r.quantity)
            .sum(),
        Err(e @ (crate::Error::Io(_) | crate::Error::Protocol(_))) => return Err(e),
        Err(_) => 0,
    };
    let want = spare_wanted(in_hold);
    let held = conn
        .hangar_items()?
        .into_iter()
        .filter(|r| is_probe(r) && r.type_id == kind)
        .min_by_key(|r| r.item_id);
    if let (Some(row), true) = (held, want > 0) {
        match conn.load_cargo(row.item_id, Some(row.quantity.min(want))) {
            Ok(_) => note.push("spare probes in the hold".to_owned()),
            Err(e @ (crate::Error::Io(_) | crate::Error::Protocol(_))) => return Err(e),
            Err(e) => note.push(format!("spare probes: {e}")),
        }
    }
    Ok(if note.is_empty() {
        String::new()
    } else {
        format!("camp: {}", note.join("; "))
    })
}

/// A launcher as the driver reads it: the fitted row, its default effect and
/// the probes in it.
pub fn launcher_of(
    row: &ItemRow,
    effect: (&'static str, i64),
    loaded: Option<(i64, i64)>,
    remembered: Option<i64>,
    active: bool,
) -> Launcher {
    Launcher {
        item_id: row.item_id,
        type_id: row.type_id,
        flag_id: row.flag_id,
        effect,
        loaded: loaded.map(|(_, n)| n),
        probe: loaded.map(|(t, _)| t).or(remembered),
        active,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{ParkView, Policy, ShipInfo};
    use crate::hunt::{Ground, Pilot};
    use eve_server::group_contacts::Affiliation;
    use std::sync::Arc;

    const SHIP: i64 = 1_000_101;
    const SYSTEM: i64 = 1_000_001;
    const LAUNCHER: i64 = 1_000_201;
    const PROBE_ITEM: i64 = 1_000_301;

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 50.0,
        }
    }

    fn row(item: i64, group: i64) -> ItemRow {
        ItemRow {
            item_id: item,
            type_id: 1_000_900 + item,
            owner_id: 0,
            location_id: SHIP,
            flag_id: 27,
            quantity: 5,
            group_id: group,
            category_id: 7,
            singleton: false,
        }
    }

    fn launcher(loaded: i64) -> Launcher {
        launcher_of(
            &row(LAUNCHER, GROUP_LAUNCHER),
            ("useMissiles", 101),
            Some((1_000_950, loaded)),
            None,
            false,
        )
    }

    fn field() -> KitModule {
        KitModule {
            item_id: 1_000_202,
            type_id: 1_000_902,
            effect: ("warpDisruptSphere", eve_server::bubble::EFFECT_FIELD),
            active: false,
            online: true,
            range: 20_000.0,
            cycle: Duration::from_secs(10),
            reactivation: Duration::ZERO,
        }
    }

    fn friend(id: i64) -> Pilot {
        Pilot {
            ball: ball(id, 1_000.0),
            char_id: id + 5,
            type_id: None,
            who: Affiliation {
                character_id: id + 5,
                corporation_id: 1_000_700,
                alliance_id: None,
            },
            hostile: false,
            relation: 0.0,
            war: false,
        }
    }

    /// A dictor in a null system with a loaded launcher.
    fn setup(now: Instant) -> (Memory, Observation) {
        let mut policy = Policy::fast();
        policy.pvp = Some(Arc::new(Ground::new().with_security(SYSTEM, -0.5)));
        let mut m = Memory::new("Dictor1", policy);
        m.combat.role = Some(Role::Interdict);
        let mut o = Observation::empty(now);
        o.place = Place::InSpace {
            system: SYSTEM,
            bound: true,
        };
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: None,
        });
        o.park = Some(ParkView {
            ego: ball(SHIP, 0.0),
            ego_warping: false,
            rocks: Vec::new(),
            belts: Vec::new(),
            station: None,
            boss: None,
            taken: Default::default(),
            hostiles: Vec::new(),
            wrecks: Vec::new(),
            ego_tether: None,
        });
        o.kit.launcher = Some(launcher(2));
        (m, o)
    }

    #[test]
    fn a_dev_dictor_stays_docked_until_its_launcher_is_loaded() {
        let t = Instant::now();
        let (mut m, mut o) = setup(t);
        m.policy.dev = Some(Arc::new(crate::dev::DevBot {
            group: "invented".to_owned(),
            ticker: "INV".to_owned(),
            skills: crate::dev::Skills::All5,
            skill_ids: None,
            hull: None,
            home: None,
            boss: false,
            hall: Arc::default(),
        }));
        o.kit.launcher = None;
        assert!(kit_pending(&mut m, &o), "no launcher yet");
        o.kit.launcher = Some(launcher(0));
        assert!(kit_pending(&mut m, &o), "fitted but empty");
        assert!(m.camp.dock_next.is_none(), "the dock-side look is due");
        o.kit.launcher = Some(launcher(2));
        assert!(
            !kit_pending(&mut m, &o),
            "#18: loaded is the dictor's whole kit, no unit asked"
        );
        // No launcher ever comes: the wait ends.
        o.kit.launcher = None;
        o.now = t + KIT_HOLD;
        assert!(!kit_pending(&mut m, &o));
        // A bot that is no dictor, or no dev bot, is never held.
        let (mut plain, o2) = setup(t);
        let mut o3 = o2.clone();
        o3.kit.launcher = None;
        assert!(!kit_pending(&mut plain, &o3), "market bots buy their part");
    }

    /// B3: a heavy interdictor is ready with its field fitted, no launcher
    /// asked; a dictor still waits for a loaded launcher.
    #[test]
    fn a_dev_hic_is_ready_with_a_field_and_a_dictor_only_with_a_loaded_launcher() {
        const HIC_TYPE: i64 = 1_000_777;
        const HIC_GROUP: i64 = crate::counter::BUBBLE_GROUPS[1];
        let t = Instant::now();
        let (mut m, mut o) = setup(t);
        m.policy.pvp = Some(Arc::new(
            Ground::new()
                .with_security(SYSTEM, -0.5)
                .with_fit(HIC_TYPE, HIC_GROUP, 0, 0),
        ));
        m.policy.dev = Some(Arc::new(crate::dev::DevBot {
            group: "invented".to_owned(),
            ticker: "INV".to_owned(),
            skills: crate::dev::Skills::All5,
            skill_ids: None,
            hull: None,
            home: None,
            boss: false,
            hall: Arc::default(),
        }));
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(HIC_TYPE),
        });
        assert!(flies_hic(&m, &o));
        o.kit.launcher = None;
        o.kit.field = None;
        assert!(kit_pending(&mut m, &o), "no field yet");
        o.kit.field = Some(field());
        assert!(
            !kit_pending(&mut m, &o),
            "a field is the kit while the bundle names no script"
        );
        // #18: the bundle names a script: the HIC waits for it in the hold.
        m.camp.script_type = Some(SCRIPT_TYPE);
        assert!(kit_pending(&mut m, &o), "a field, the script not aboard");
        o.cargo = vec![script_row(SCRIPT_ITEM)];
        assert!(
            !kit_pending(&mut m, &o),
            "a field and its script are the HIC's kit"
        );
        o.cargo.clear();
        m.camp.script_type = None;
        m.camp.kit_wait_from = None;
        // The same kit on a hull that is no HIC does not release a dictor.
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(HIC_TYPE + 1),
        });
        m.camp.kit_wait_from = None;
        assert!(kit_pending(&mut m, &o), "a dictor waits for its launcher");
    }

    /// B3: a hangar row is a field generator by the group the bundle gave.
    #[test]
    fn a_field_generator_row_is_known_by_the_bundle_group() {
        const FIELD_GROUP: i64 = 1_000_899;
        let (mut m, _) = setup(Instant::now());
        let r = ItemRow {
            item_id: 5,
            type_id: 1_000_903,
            owner_id: 1,
            location_id: 2,
            flag_id: crate::FLAG_HANGAR,
            quantity: 1,
            group_id: FIELD_GROUP,
            category_id: 0,
            singleton: true,
        };
        assert!(!is_field(&m, &r), "no group read yet");
        m.camp.field_group = Some(FIELD_GROUP);
        assert!(is_field(&m, &r));
        assert!(!is_field(&m, &ItemRow { group_id: 0, ..r }));
    }

    #[test]
    fn a_dictor_launches_on_a_hostile_in_reach_once_per_bubble() {
        let t = Instant::now();
        let (mut m, o) = setup(t);
        let prey = ball(9, LAUNCH_REACH / 2.0);
        assert!(matches!(
            launch_now(&m, &o, &prey),
            Some(Act::Kit(KitAct::Launch(_)))
        ));
        // Out of reach: nothing.
        assert_eq!(launch_now(&m, &o, &ball(9, LAUNCH_REACH * 2.0)), None);
        // One was just launched: nothing until the next is due.
        m.camp.launched_at = Some(t);
        assert!(m.camp.own_probe_alive(t));
        assert_eq!(launch_now(&m, &o, &prey), None);
        let mut later = o.clone();
        later.now = t + BUBBLE_EVERY;
        assert!(!m.camp.own_probe_alive(later.now));
        assert!(launch_now(&m, &later, &prey).is_some());
    }

    #[test]
    fn a_covered_prey_gets_no_second_probe() {
        let t = Instant::now();
        let (m, mut o) = setup(t);
        let prey = ball(9, 5_000.0);
        o.warp_bubble.zones = vec![([4_000.0, 0.0, 0.0], 10_000.0)];
        assert_eq!(launch_now(&m, &o, &prey), None);
        o.warp_bubble.zones = vec![([-90_000.0, 0.0, 0.0], 10_000.0)];
        assert!(launch_now(&m, &o, &prey).is_some());
    }

    #[test]
    fn nothing_is_launched_without_probes_a_role_or_a_band() {
        let t = Instant::now();
        let prey = ball(9, 1_000.0);
        let (m, mut o) = setup(t);
        o.kit.launcher = Some(launcher(0));
        assert_eq!(launch_now(&m, &o, &prey), None, "an empty launcher");
        let (mut m, o) = setup(t);
        m.combat.role = Some(Role::Dps);
        assert_eq!(launch_now(&m, &o, &prey), None, "not an interdictor");
        // Low sec: the server refuses, so the bot never asks.
        let (mut m, o) = setup(t);
        let mut p = Policy::fast();
        p.pvp = Some(Arc::new(Ground::new().with_security(SYSTEM, 0.3)));
        m.policy = p;
        assert_eq!(launch_now(&m, &o, &prey), None, "low sec");
        // No map of the system at all: also no.
        m.policy.pvp = None;
        assert_eq!(launch_now(&m, &o, &prey), None, "unknown band");
        // A refused launch rests the launcher.
        let (mut m, o) = setup(t);
        m.camp.blocked(t);
        assert_eq!(launch_now(&m, &o, &prey), None);
    }

    #[test]
    fn an_empty_launcher_is_refilled_from_the_hold() {
        let t = Instant::now();
        let (mut m, mut o) = setup(t);
        o.kit.launcher = Some(launcher(0));
        assert_eq!(reload_now(&mut m, &o), None, "no probe in the hold");
        o.cargo = vec![row(PROBE_ITEM, GROUP_PROBE), row(PROBE_ITEM + 1, 83)];
        let Some(Act::LoadAmmo {
            guns,
            charges,
            location,
        }) = reload_now(&mut m, &o)
        else {
            panic!("no reload");
        };
        assert_eq!(guns, vec![(LAUNCHER, 27)]);
        assert_eq!(charges, vec![PROBE_ITEM], "only probes");
        assert_eq!(location, SHIP);
        assert_eq!(reload_now(&mut m, &o), None, "locked out");
    }

    #[test]
    fn a_hic_runs_its_field_while_the_prey_is_inside_and_friends_are_there() {
        let t = Instant::now();
        let (m, mut o) = setup(t);
        o.kit.launcher = None;
        o.kit.field = Some(field());
        let range = o.kit.field.unwrap().range;
        let inside = ball(9, range / 2.0);
        assert_eq!(field_now(&m, &o, &inside), None, "alone");
        o.hunt.pilots = vec![friend(20)];
        assert!(matches!(
            field_now(&m, &o, &inside),
            Some(Act::Kit(KitAct::FieldOn(_)))
        ));
        // At the edge, inside the margin: no.
        assert_eq!(
            field_now(&m, &o, &ball(9, range - FIELD_MARGIN / 2.0)),
            None
        );
        // Running: stops when the prey is gone or out of reach, or on the way home.
        let mut on = o.clone();
        on.kit.field = Some(KitModule {
            active: true,
            ..field()
        });
        assert_eq!(field_off(&m, &on, Some(&inside)), None);
        assert!(field_off(&m, &on, None).is_some());
        assert!(field_off(&m, &on, Some(&ball(9, range * 2.0))).is_some());
        let mut homing = m.clone();
        homing.homing = Some(crate::brain::Homing::Repair);
        assert!(field_off(&homing, &on, Some(&inside)).is_some());
        assert_eq!(field_now(&homing, &o, &inside), None);
    }

    /// The fight is where it runs: a hostile pilot in reach makes the real
    /// `hunt::fight` carry the launch, once.
    #[test]
    fn the_hunt_fight_carries_the_launch() {
        let t = Instant::now();
        let (mut m, mut o) = setup(t);
        let prey = Pilot {
            hostile: true,
            ..friend(30)
        };
        let park = o.park.clone().unwrap();
        o.hunt.pilots = vec![Pilot {
            ball: ball(30, 6_000.0),
            ..prey.clone()
        }];
        let target = o.hunt.pilots[0].clone();
        let acts = crate::hunt::fight(&mut m, &o, &park, &target, Vec::new());
        assert!(
            acts.iter()
                .any(|a| matches!(a, Act::Kit(KitAct::Launch(_)))),
            "{acts:?}"
        );
    }

    #[test]
    fn a_running_field_is_stopped_at_the_top_of_the_turn() {
        let t = Instant::now();
        let (m, mut o) = setup(t);
        o.kit.launcher = None;
        o.kit.field = Some(KitModule {
            active: true,
            ..field()
        });
        o.hunt.pilots = vec![friend(20)];
        assert!(field_check(&m, &o).is_some(), "no hostile left");
        o.hunt.pilots.push(Pilot {
            hostile: true,
            ..friend(30)
        });
        assert_eq!(field_check(&m, &o), None, "prey inside, a friend on grid");
        o.hunt.pilots[1].ball = ball(30, 1.0e6);
        assert!(field_check(&m, &o).is_some(), "prey out of reach");
        let mut other = m.clone();
        other.combat.role = Some(Role::Dps);
        assert_eq!(field_check(&other, &o), None);
    }

    #[test]
    fn a_camper_takes_the_lowest_gate_once_and_holds_it() {
        use crate::travel::Map;
        let t = Instant::now();
        let (mut m, mut o) = setup(t);
        // No map: no gate to take.
        let park = o.park.clone().unwrap();
        assert_eq!(gate_of(&m, SYSTEM), None);
        assert_eq!(post(&mut m, &o, &park, SYSTEM), None);
        // Two neighbours: the lower-numbered system's gate.
        let map = Map::new()
            .with_gate(SYSTEM, 2_000_501, 1_000_099, 2_000_502)
            .with_gate(SYSTEM, 2_000_503, 1_000_098, 2_000_504);
        m.policy.travel = Some(Arc::new(map));
        assert_eq!(gate_of(&m, SYSTEM), Some(2_000_503));
        let Some(Act::WarpToItem { id, range }) = post(&mut m, &o, &park, SYSTEM) else {
            panic!("no warp");
        };
        assert_eq!((id, range), (2_000_503, GATE_STANDOFF));
        // Posted and the warp over (the gate on grid, near): it stays,
        // however long it waits.
        o.warp_bubble.gates = vec![ball(2_000_503, 1_000.0)];
        o.now = t + POST_EVERY * 3;
        assert_eq!(post(&mut m, &o, &park, SYSTEM), None);
        // The station is a global ball, in this park too: it changes nothing.
        let mut home = park.clone();
        home.station = Some(ball(7, 100.0));
        // The gate on grid, near: no warp, and the camper holds.
        o.warp_bubble.gates = vec![ball(2_000_503, 1_000.0)];
        o.now = t + POST_EVERY * 6;
        assert_eq!(post(&mut m, &o, &home, SYSTEM), None, "near the gate");
        // Far from the gate (back from a dock), before the pause: paused;
        // after it, sent again.
        o.warp_bubble.gates = vec![ball(2_000_503, 1.0e7)];
        m.camp.post_next = Some(o.now + Duration::from_secs(1));
        assert_eq!(post(&mut m, &o, &home, SYSTEM), None, "paused");
        o.now += POST_EVERY * 3;
        assert!(matches!(
            post(&mut m, &o, &home, SYSTEM),
            Some(Act::WarpToItem { .. })
        ));
        // In warp: nothing.
        o.now += POST_EVERY * 3;
        let mut warping = home.clone();
        warping.ego_warping = true;
        assert_eq!(post(&mut m, &o, &warping, SYSTEM), None, "in warp");
        // Not in warp, and not where no bubble may be made.
        let mut low = m.clone();
        low.camp = Mem::default();
        low.policy.pvp = Some(Arc::new(Ground::new().with_security(SYSTEM, 0.3)));
        assert_eq!(post(&mut low, &o, &park, SYSTEM), None, "low sec");
    }

    #[test]
    fn a_held_position_keeps_the_shots_and_drops_the_moves() {
        let acts = vec![
            Act::Orbit { id: 1, range: 1.0 },
            Act::Follow { id: 1, range: 1.0 },
            Act::WarpToItem { id: 1, range: 1.0 },
            Act::WarpToMember {
                char_id: 1,
                range: 1.0,
            },
            Act::Fire { gun: 2, target: 3 },
            Act::Lock(3),
        ];
        assert_eq!(
            hold_position(acts),
            vec![Act::Fire { gun: 2, target: 3 }, Act::Lock(3)]
        );
    }

    #[test]
    fn the_hunt_fight_of_a_camper_never_orbits() {
        let t = Instant::now();
        let (mut m, mut o) = setup(t);
        let park = o.park.clone().unwrap();
        o.guns = vec![crate::brain::GunInfo {
            item_id: 1_000_401,
            type_id: 1_000_901,
            flag_id: 27,
            online: true,
            active: false,
            target: None,
            rounds: Some(10),
            capacity: 10,
            range: 2_000.0,
            reload: Duration::ZERO,
            kind: crate::brain::GunKind::Weapon,
            reach: 0.0,
        }];
        o.hunt.pilots = vec![Pilot {
            ball: ball(30, 6_000.0),
            hostile: true,
            ..friend(30)
        }];
        let target = o.hunt.pilots[0].clone();
        let acts = crate::hunt::fight(&mut m, &o, &park, &target, Vec::new());
        assert!(
            !acts
                .iter()
                .any(|a| matches!(a, Act::Orbit { .. } | Act::Follow { .. })),
            "{acts:?}"
        );
        // Anyone else still closes in.
        m.combat.role = Some(Role::Dps);
        let acts = crate::hunt::fight(&mut m, &o, &park, &target, Vec::new());
        assert!(
            acts.iter()
                .any(|a| matches!(a, Act::Orbit { .. } | Act::Follow { .. })),
            "{acts:?}"
        );
    }

    #[test]
    fn the_hold_is_topped_up_to_the_spare_not_loaded_again() {
        assert_eq!(spare_wanted(0), SPARE_PROBES);
        assert_eq!(spare_wanted(SPARE_PROBES - 1), 1);
        assert_eq!(spare_wanted(SPARE_PROBES), 0);
        assert_eq!(spare_wanted(SPARE_PROBES + 4), 0);
    }

    #[test]
    fn only_an_interdictor_camps() {
        let t = Instant::now();
        let (mut m, mut o) = setup(t);
        o.hunt.pilots = vec![friend(20)];
        o.cargo = vec![row(PROBE_ITEM, GROUP_PROBE)];
        o.kit.launcher = Some(launcher(0));
        assert!(!acts(&mut m, &o, &ball(9, 1_000.0)).is_empty());
        let (mut m, _) = setup(t);
        m.combat.role = Some(Role::Logi);
        assert!(acts(&mut m, &o, &ball(9, 1_000.0)).is_empty());
        assert!(quiet(&mut m, &o).is_empty());
    }

    // ---- NW-11b: the camp's own mobile warp disruptor ----

    const SCRIPT_TYPE: i64 = 1_000_951;
    const SCRIPT_ITEM: i64 = 1_000_551;

    /// A script row (invented ids; group 908).
    fn script_row(item: i64) -> ItemRow {
        ItemRow {
            type_id: SCRIPT_TYPE,
            ..row(item, GROUP_SCRIPT)
        }
    }

    const GATE: i64 = 1_000_401;
    const UNIT_ITEM: i64 = 1_000_501;
    const UNIT_BALL: i64 = 1_000_601;

    /// A unit carrier (#18: no interdictor) standing on the gate's line with
    /// a unit in the hold.
    fn at_gate(now: Instant) -> (Memory, Observation) {
        let (mut m, mut o) = setup(now);
        m.combat.role = Some(Role::Dps);
        o.kit.launcher = None;
        // The gate at the origin, the camper at its standoff.
        o.park.as_mut().unwrap().ego = ball(SHIP, GATE_STANDOFF);
        o.warp_bubble.gates = vec![ball(GATE, 0.0)];
        o.cargo = vec![row(UNIT_ITEM, GROUP_UNIT)];
        m.camp.posted = Some((SYSTEM, GATE));
        (m, o)
    }

    fn out(stage: Stage, at: Instant) -> Unit {
        Unit {
            id: UNIT_BALL,
            stage,
            at,
            spot: [GATE_STANDOFF + DEPLOY_AT, 0.0, 0.0],
        }
    }

    #[test]
    fn a_camper_at_its_gate_with_a_unit_launches_it() {
        let (m, o) = at_gate(Instant::now());
        assert_eq!(
            unit_deploy(&m, &o),
            Some(Act::Kit(KitAct::UnitLaunch(UNIT_ITEM)))
        );
    }

    /// A gate camp sets its bubble up before anyone lands: no prey, and the
    /// seat's station on the grid (a global ball), still launches.
    #[test]
    fn a_ready_camper_launches_without_prey_and_with_a_station_on_the_grid() {
        let (m, mut o) = at_gate(Instant::now());
        assert!(o.hunt.pilots.is_empty() && o.park.as_ref().unwrap().hostiles.is_empty());
        o.park.as_mut().unwrap().station = Some(ball(7, 100.0));
        assert_eq!(unit_state(&m, &o), "ready");
        assert_eq!(
            unit_deploy(&m, &o),
            Some(Act::Kit(KitAct::UnitLaunch(UNIT_ITEM)))
        );
    }

    #[test]
    fn no_unit_is_launched_without_one_a_band_a_line_or_a_gap() {
        let t = Instant::now();
        // None in the hold: nothing from nowhere.
        let (m, mut o) = at_gate(t);
        o.cargo.clear();
        assert_eq!(unit_deploy(&m, &o), None);
        // #18: an interdictor never launches one, by role or by hull.
        let (mut m, o) = at_gate(t);
        assert!(unit_deploy(&m, &o).is_some(), "a carrier does");
        m.combat.role = Some(Role::Interdict);
        assert_eq!(unit_deploy(&m, &o), None, "the Interdict role");
        assert_eq!(unit_state(&m, &o), "interdictor carries none");
        let (mut m, mut o) = at_gate(t);
        const DICTOR_HULL: i64 = 1_000_778;
        m.policy.pvp = Some(Arc::new(
            Ground::new().with_security(SYSTEM, -0.5).with_fit(
                DICTOR_HULL,
                crate::counter::BUBBLE_GROUPS[0],
                0,
                0,
            ),
        ));
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(DICTOR_HULL),
        });
        assert_eq!(unit_deploy(&m, &o), None, "a bubble hull");
        // A heavy interdictor, the same.
        const HIC_HULL: i64 = 1_000_779;
        m.policy.pvp = Some(Arc::new(
            Ground::new().with_security(SYSTEM, -0.5).with_fit(
                HIC_HULL,
                crate::counter::BUBBLE_GROUPS[1],
                0,
                0,
            ),
        ));
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(HIC_HULL),
        });
        assert_eq!(unit_deploy(&m, &o), None, "a HIC hull");
        // High sec: no bubbles.
        let (mut m, o) = at_gate(t);
        m.policy.pvp = Some(Arc::new(Ground::new().with_security(SYSTEM, 0.9)));
        assert_eq!(unit_deploy(&m, &o), None);
        // Far off the line (still at the station side of the system).
        let (m, mut o) = at_gate(t);
        o.park.as_mut().unwrap().ego = ball(SHIP, GATE_STANDOFF + LINE_SLACK * 3.0);
        assert_eq!(unit_deploy(&m, &o), None);
        // A sphere already holds the gate (a unit anchored there).
        let (m, mut o) = at_gate(t);
        o.warp_bubble.zones = vec![([0.0, 0.0, 0.0], UNIT_RADIUS)];
        assert_eq!(unit_deploy(&m, &o), None);
        // Not posted at this gate.
        let (mut m, o) = at_gate(t);
        m.camp.posted = None;
        assert_eq!(unit_deploy(&m, &o), None);
        // In warp.
        let (m, mut o) = at_gate(t);
        o.park.as_mut().unwrap().ego_warping = true;
        assert_eq!(unit_deploy(&m, &o), None);
    }

    #[test]
    fn a_sphere_that_cannot_hold_the_gate_is_not_worth_launching() {
        assert!(unit_covers_gate(
            [GATE_STANDOFF, 0.0, 0.0],
            [0.0; 3],
            UNIT_RADIUS
        ));
        assert!(!unit_covers_gate(
            [UNIT_RADIUS, 0.0, 0.0],
            [0.0; 3],
            UNIT_RADIUS
        ));
    }

    /// An invented type: a small sphere and a long anchoring delay.
    const SMALL_RADIUS: f64 = 8_000.0;
    const LONG_DELAY: Duration = Duration::from_secs(200);

    #[test]
    fn the_read_radius_moves_the_standoff_and_decides_the_launch() {
        use crate::travel::Map;
        let t = Instant::now();
        let (mut m, mut o) = at_gate(t);
        // Defaults: the standoff and the radius are the constants.
        assert_eq!(m.camp.standoff(), GATE_STANDOFF);
        // A small sphere: the camper stands closer, never under the floor.
        m.camp.unit_radius = Some(SMALL_RADIUS);
        let near = m.camp.standoff();
        assert!(near < GATE_STANDOFF - LINE_SLACK, "the old line is off");
        assert!(near + DEPLOY_AT + UNIT_MARGIN < SMALL_RADIUS);
        m.camp.unit_radius = Some(1.0);
        assert_eq!(m.camp.standoff(), MIN_STANDOFF);
        // The camper at the old standoff is off the new line: no launch ...
        m.camp.unit_radius = Some(SMALL_RADIUS);
        assert_eq!(unit_deploy(&m, &o), None);
        // ... at the new one it launches.
        o.park.as_mut().unwrap().ego = ball(SHIP, near);
        assert_eq!(
            unit_deploy(&m, &o),
            Some(Act::Kit(KitAct::UnitLaunch(UNIT_ITEM)))
        );
        // Inside LINE_SLACK of the new line but too far out, the sphere would
        // not hold the gate: no launch.
        o.park.as_mut().unwrap().ego = ball(SHIP, near + LINE_SLACK);
        assert_eq!(unit_deploy(&m, &o), None);
        // The post warps in to the new standoff.
        let (mut m2, mut o2) = setup(t);
        m2.camp.unit_radius = Some(SMALL_RADIUS);
        m2.policy.travel = Some(Arc::new(
            Map::new().with_gate(SYSTEM, GATE, 1_000_098, 2_000_504),
        ));
        o2.warp_bubble.gates = vec![ball(GATE, 1.0e7)];
        let park = o2.park.clone().unwrap();
        let Some(Act::WarpToItem { range, .. }) = post(&mut m2, &o2, &park, SYSTEM) else {
            panic!("no warp");
        };
        assert_eq!(range, m2.camp.standoff());
    }

    #[test]
    fn a_camper_that_arrived_off_the_line_closes_in_and_a_big_gate_is_said_once() {
        use crate::travel::Map;
        let t = Instant::now();
        let (mut m, mut o) = at_gate(t);
        m.policy.travel = Some(Arc::new(
            Map::new().with_gate(SYSTEM, GATE, 1_000_098, 2_000_504),
        ));
        // At the gate, on grid, but at the jump-in spot (too close), with a
        // unit in the hold: it keeps range at the standoff, no oftener than
        // the post pause.
        o.park.as_mut().unwrap().ego = ball(SHIP, 1_500.0);
        let park = o.park.clone().unwrap();
        assert_eq!(
            post(&mut m, &o, &park, SYSTEM),
            Some(Act::Follow {
                id: GATE,
                range: m.camp.standoff()
            })
        );
        assert_eq!(post(&mut m, &o, &park, SYSTEM), None, "paused");
        // On the line: nothing to do.
        o.now = t + POST_EVERY * 2;
        o.park.as_mut().unwrap().ego = ball(SHIP, m.camp.standoff());
        let park = o.park.clone().unwrap();
        assert_eq!(post(&mut m, &o, &park, SYSTEM), None);
        // No unit in the hold: it just holds.
        o.cargo.clear();
        o.park.as_mut().unwrap().ego = ball(SHIP, 1_500.0);
        let park = o.park.clone().unwrap();
        o.now = t + POST_EVERY * 4;
        assert_eq!(post(&mut m, &o, &park, SYSTEM), None);
        // A gate so big that its landing point is outside the sphere.
        o.cargo = vec![row(UNIT_ITEM, GROUP_UNIT)];
        let mut big = ball(GATE, 0.0);
        big.radius = UNIT_RADIUS;
        assert!(gate_too_big(&m, &big));
        assert!(!gate_too_big(&m, &ball(GATE, 0.0)));
        // The rule has an edge: a landing point plus offset and margin just
        // inside the sphere is fine, just outside is too big (invented radii).
        let edge = UNIT_RADIUS - eve_server::space_state::WARP_STANDOFF - DEPLOY_AT - UNIT_MARGIN;
        let mut g = ball(GATE, 0.0);
        g.radius = edge - 1.0;
        assert!(!gate_too_big(&m, &g));
        g.radius = edge;
        assert!(gate_too_big(&m, &g));
        o.warp_bubble.gates = vec![big];
        o.now = t + POST_EVERY * 6;
        assert_eq!(post(&mut m, &o, &park, SYSTEM), None);
        assert!(m.camp.big_gate_logged);
    }

    /// The unit type's sphere radius and delay come from the bundle by the
    /// type's name; invented numbers.
    #[test]
    fn the_unit_specs_are_read_from_the_bundle_by_name() {
        use eve_server::content::Content;
        let rows = format!(
            "{}type\t1000777\t{GROUP_UNIT}\t22\t1\t1\t10.0\t0.0\t0.0\t1.0\t0\t\t\
             103=7777.0;556=123000.0\t{UNIT_NAME}\n",
            eve_server::typedata::FIXTURE
        );
        let mut c = Content::fixture();
        let table = eve_server::typedata::Table::parse(&rows).expect("rows parse");
        c.insert("typedata", eve_server::typedata::Installed::build(table));
        assert_eq!(
            unit_specs_in(&c),
            (Some(7777.0), Some(Duration::from_secs(123)))
        );
        // The bare fixture has no such type.
        assert_eq!(unit_specs_in(&Content::fixture()), (None, None));
        // A type without the attributes leaves both to the constants.
        let bare = rows.replace("103=7777.0;556=123000.0", "");
        let mut d = Content::fixture();
        d.insert(
            "typedata",
            eve_server::typedata::Installed::build(
                eve_server::typedata::Table::parse(&bare).expect("rows parse"),
            ),
        );
        assert_eq!(unit_specs_in(&d), (None, None));
    }

    #[test]
    fn the_unit_state_says_why_it_is_or_is_not_going_out() {
        let t = Instant::now();
        let (mut m, mut o) = at_gate(t);
        assert_eq!(unit_state(&m, &o), "ready");
        o.cargo.clear();
        assert_eq!(unit_state(&m, &o), "none in hold");
        o.cargo = vec![row(UNIT_ITEM, GROUP_UNIT)];
        o.park.as_mut().unwrap().ego = ball(SHIP, 1_000.0);
        assert_eq!(unit_state(&m, &o), "off line");
        let mut big = ball(GATE, 0.0);
        big.radius = UNIT_RADIUS;
        o.warp_bubble.gates = vec![big];
        assert_eq!(unit_state(&m, &o), "gate too big");
        m.camp.unit = Some(out(Stage::Anchoring, t));
        assert_eq!(unit_state(&m, &o), "anchoring");
        // One log line per change: the note follows the state.
        note_unit(&mut m, &o);
        assert_eq!(m.camp.unit_note, Some("anchoring"));
    }

    #[test]
    fn a_pilot_beyond_safe_range_is_no_prey_for_a_camper() {
        let me = ball(SHIP, 0.0);
        assert!(prey_in_reach(&me, &ball(40, SAFE_RANGE - 1.0)));
        assert!(!prey_in_reach(&me, &ball(40, SAFE_RANGE + 1_000.0)));
    }

    #[test]
    fn the_read_delay_times_the_anchor_and_the_lift() {
        let t = Instant::now();
        let (mut m, mut o) = at_gate(t);
        m.camp.unit_delay = Some(LONG_DELAY);
        assert_eq!(m.camp.anchor_wait(), LONG_DELAY);
        assert_eq!(m.camp.lift_wait(), LONG_DELAY);
        o.warp_bubble.zones = vec![([GATE_STANDOFF + DEPLOY_AT, 0.0, 0.0], UNIT_RADIUS)];
        m.camp.unit = Some(out(Stage::Anchoring, t));
        // The default wait has passed, the delay has not: stay on the unit.
        o.now = t + ANCHOR_WAIT + Duration::from_secs(1);
        assert_eq!(
            unit_retrieve(&mut m, &o, true),
            Some(Act::Wait(crate::brain::SHORT_WAIT))
        );
        o.now = t + LONG_DELAY;
        assert_eq!(
            unit_retrieve(&mut m, &o, true),
            Some(Act::Kit(KitAct::UnitLift(UNIT_BALL)))
        );
        m.camp.unit = Some(out(Stage::Lifting, t));
        o.now = t + LIFT_WAIT + Duration::from_secs(1);
        assert_eq!(
            unit_retrieve(&mut m, &o, true),
            Some(Act::Wait(crate::brain::SHORT_WAIT))
        );
        o.now = t + LONG_DELAY;
        assert_eq!(
            unit_retrieve(&mut m, &o, true),
            Some(Act::Kit(KitAct::UnitScoop(UNIT_BALL)))
        );
    }

    #[test]
    fn a_launched_unit_is_anchored_once_and_a_refusal_rests_it() {
        let t = Instant::now();
        let (mut m, o) = at_gate(t);
        m.camp.unit = Some(out(Stage::Launched, t));
        assert_eq!(
            unit_deploy(&m, &o),
            Some(Act::Kit(KitAct::UnitAnchor(UNIT_BALL)))
        );
        m.camp.unit = Some(out(Stage::Anchoring, t));
        assert_eq!(unit_deploy(&m, &o), None, "anchoring: nothing to do");
        m.camp.unit = Some(out(Stage::Launched, t));
        m.camp.unit_blocked_until = Some(t + UNIT_BACKOFF);
        assert_eq!(unit_deploy(&m, &o), None);
    }

    #[test]
    fn the_unit_stays_while_the_camp_lasts_and_comes_back_when_it_ends() {
        let t = Instant::now();
        let later = t + ANCHOR_WAIT + Duration::from_secs(1);
        let (mut m, mut o) = at_gate(t);
        o.now = later;
        o.warp_bubble.zones = vec![([GATE_STANDOFF + DEPLOY_AT, 0.0, 0.0], UNIT_RADIUS)];
        m.camp.unit = Some(out(Stage::Anchoring, t));
        assert_eq!(unit_retrieve(&mut m, &o, false), None, "camp on: stays");
        assert_eq!(
            unit_retrieve(&mut m, &o, true),
            Some(Act::Kit(KitAct::UnitLift(UNIT_BALL)))
        );
        // Not before its anchor has settled: the camper waits on its unit.
        o.now = t + Duration::from_secs(1);
        assert_eq!(
            unit_retrieve(&mut m, &o, true),
            Some(Act::Wait(crate::brain::SHORT_WAIT))
        );
        assert_eq!(unit_retrieve(&mut m, &o, false), None, "camp on: stays");
        // Lifting: scooped after the wait only.
        m.camp.unit = Some(out(Stage::Lifting, t));
        assert_eq!(
            unit_retrieve(&mut m, &o, true),
            Some(Act::Wait(crate::brain::SHORT_WAIT))
        );
        o.now = t + LIFT_WAIT;
        assert_eq!(
            unit_retrieve(&mut m, &o, true),
            Some(Act::Kit(KitAct::UnitScoop(UNIT_BALL)))
        );
        // A free unit (never anchored) is scooped at once.
        m.camp.unit = Some(out(Stage::Launched, t));
        assert_eq!(
            unit_retrieve(&mut m, &o, true),
            Some(Act::Kit(KitAct::UnitScoop(UNIT_BALL)))
        );
    }

    #[test]
    fn the_unit_is_not_taken_back_under_the_enemys_guns_or_from_afar() {
        let t = Instant::now();
        let (mut m, mut o) = at_gate(t);
        o.warp_bubble.zones = vec![([GATE_STANDOFF + DEPLOY_AT, 0.0, 0.0], UNIT_RADIUS)];
        m.camp.unit = Some(out(Stage::Launched, t));
        let mut foe = friend(40);
        foe.hostile = true;
        foe.ball = ball(40, GATE_STANDOFF + 1_000.0);
        o.hunt.pilots = vec![foe];
        assert_eq!(unit_retrieve(&mut m, &o, true), None, "hostile near");
        o.hunt.pilots.clear();
        // Far away: it flies there first.
        o.park.as_mut().unwrap().ego = ball(SHIP, GATE_STANDOFF + 40.0 * WORK_RANGE);
        assert!(matches!(
            unit_retrieve(&mut m, &o, true),
            Some(Act::WarpToItem { id: UNIT_BALL, .. })
        ));
    }

    #[test]
    fn a_unit_that_is_gone_is_forgotten() {
        let t = Instant::now();
        let (mut m, mut o) = at_gate(t);
        o.warp_bubble.zones.clear();
        m.camp.unit = Some(out(Stage::Anchoring, t));
        assert_eq!(unit_retrieve(&mut m, &o, false), None);
        assert!(m.camp.unit.is_some(), "unseen for a pass: kept");
        o.now = t + UNSEEN_GRACE;
        assert_eq!(unit_retrieve(&mut m, &o, false), None);
        assert_eq!(m.camp.unit, None, "shot: no sphere at its spot for long");
        // The same, but the bot is elsewhere: it cannot tell, so it keeps it.
        let (mut m, mut o) = at_gate(t);
        o.park.as_mut().unwrap().ego = ball(SHIP, 100.0 * WORK_RANGE);
        m.camp.unit = Some(out(Stage::Anchoring, t));
        assert_eq!(unit_retrieve(&mut m, &o, false), None);
        assert!(m.camp.unit.is_some());
    }

    /// The lift reaches the client as a removal and a re-add: the unit is
    /// missing for a pass, and the camper must still scoop it after the wait.
    #[test]
    fn a_lifting_unit_unseen_for_a_pass_is_still_scooped_after_the_lift() {
        let t = Instant::now();
        let (mut m, mut o) = at_gate(t);
        o.warp_bubble.zones.clear();
        m.camp.unit = Some(out(Stage::Lifting, t));
        o.now = t + Duration::from_secs(1);
        assert_eq!(
            unit_retrieve(&mut m, &o, true),
            Some(Act::Wait(crate::brain::SHORT_WAIT))
        );
        // Unseen well past the plain grace, still inside the lift wait: kept.
        o.now = t + UNSEEN_GRACE * 2;
        assert!(unit_retrieve(&mut m, &o, true).is_some());
        assert!(m.camp.unit.is_some());
        // The lift is done and the ball is back: scoop.
        o.now = t + LIFT_WAIT;
        o.warp_bubble.zones = vec![([GATE_STANDOFF + DEPLOY_AT, 0.0, 0.0], UNIT_RADIUS)];
        assert_eq!(
            unit_retrieve(&mut m, &o, true),
            Some(Act::Kit(KitAct::UnitScoop(UNIT_BALL)))
        );
        assert_eq!(m.camp.unit_unseen, None);
    }

    /// SOAK-S4: an offline field generator, or one in its back-off, is not
    /// sent; a stop is not repeated every turn.
    #[test]
    fn an_offline_or_backed_off_field_is_not_run() {
        let t = Instant::now();
        let (mut m, mut o) = setup(t);
        o.kit.launcher = None;
        o.kit.field = Some(field());
        o.hunt.pilots = vec![friend(20)];
        let inside = ball(9, 1_000.0);
        assert!(field_now(&m, &o, &inside).is_some());
        m.camp.field_blocked(t);
        assert_eq!(field_now(&m, &o, &inside), None, "backed off");
        o.now = t + BLOCKED_BACKOFF + Duration::from_secs(1);
        assert!(field_now(&m, &o, &inside).is_some(), "back-off over");
        o.kit.field = Some(KitModule {
            online: false,
            ..field()
        });
        assert_eq!(field_now(&m, &o, &inside), None, "offline");
        // A stop just sent is not sent again until the pause is over.
        o.kit.field = Some(KitModule {
            active: true,
            ..field()
        });
        assert!(field_off(&m, &o, None).is_some());
        m.camp.field_off_until = Some(o.now + FIELD_OFF_EVERY);
        assert_eq!(field_off(&m, &o, None), None);
    }

    // ---- #18: each hull's own tackle ----

    /// The unit's carrier is a dev bot named for the stem, never an interdictor
    /// that was granted one; it waits at the dock for the unit in its hold.
    #[test]
    fn the_unit_carrier_waits_for_its_unit_and_no_interdictor_does() {
        let t = Instant::now();
        let dev = || {
            Some(Arc::new(crate::dev::DevBot {
                group: "invented".to_owned(),
                ticker: "INV".to_owned(),
                skills: crate::dev::Skills::All5,
                skill_ids: None,
                hull: None,
                home: None,
                boss: false,
                hall: Arc::default(),
            }))
        };
        let (mut m, mut o) = at_gate(t);
        o.cargo.clear();
        m.policy.dev = dev();
        assert!(!is_carrier(&m), "no stem in the name");
        assert!(
            !kit_pending(&mut m, &o),
            "a dps bot with no part is not held"
        );
        // The names the log showed: the side's prefix, the stem, the numbers.
        m.name = format!("Red{CARRIER_STEM}1x1");
        assert!(is_carrier(&m));
        m.name = format!("{CARRIER_STEM}2x1");
        assert!(is_carrier(&m), "no side prefix");
        for name in [
            "Anchorsmith",
            "RedAnchorsmith",
            "Anchors",
            "RedHics1x1",
            "XAnchors1x1",
        ] {
            m.name = name.to_owned();
            assert!(!is_carrier(&m), "{name}");
        }
        m.name = format!("Red{CARRIER_STEM}1x1");
        assert!(kit_pending(&mut m, &o), "the unit is not in the hold");
        o.cargo = vec![row(UNIT_ITEM, GROUP_UNIT)];
        assert!(!kit_pending(&mut m, &o), "the unit aboard: go");
        // The stem without a dev bot is no carrier.
        m.policy.dev = None;
        assert!(!is_carrier(&m));
    }

    /// The open-mode grants, from invented bundle rows: a dictor gets its
    /// launcher and probes, a HIC its generator and script, **neither a
    /// unit**; the carrier gets the unit alone.
    #[test]
    fn the_grants_give_an_interdictor_no_unit_and_the_carrier_only_the_unit() {
        use eve_server::content::Content;
        let row = |id: i64, group: i64, name: &str| {
            format!("type\t{id}\t{group}\t22\t1\t1\t10.0\t0.0\t0.0\t1.0\t0\t\t\t{name}\n")
        };
        let rows = format!(
            "{}{}{}{}{}{}",
            eve_server::typedata::FIXTURE,
            row(1_000_801, GROUP_LAUNCHER, LAUNCHER_NAME),
            row(1_000_802, GROUP_PROBE, PROBE_NAME),
            row(1_000_803, 1_000_899, FIELD_NAME),
            row(1_000_804, GROUP_SCRIPT, SCRIPT_NAME),
            row(1_000_805, GROUP_UNIT, UNIT_NAME),
        );
        let mut c = Content::fixture();
        let table = eve_server::typedata::Table::parse(&rows).expect("rows parse");
        c.insert("typedata", eve_server::typedata::Installed::build(table));
        let dictor = gm_lines(&c, false);
        assert!(dictor.iter().any(|l| l.starts_with("/createitem 1000801")));
        assert!(dictor.iter().any(|l| l.starts_with("/createitem 1000802")));
        assert!(
            !dictor.iter().any(|l| l.contains("1000805")),
            "no unit: {dictor:?}"
        );
        let hic = gm_lines(&c, true);
        assert!(hic.iter().any(|l| l == "/createitem 1000803"));
        assert!(hic.iter().any(|l| l == "/createitem 1000804"), "the script");
        assert!(
            !hic.iter().any(|l| l.contains("1000805")),
            "no unit: {hic:?}"
        );
        assert_eq!(gm_lines_carrier(&c), vec!["/createitem 1000805".to_owned()]);
        assert_eq!(script_type_in(&c), Some(1_000_804));
        // Without the types the bundle grants nothing.
        let bare = Content::fixture();
        assert!(gm_lines(&bare, true).is_empty() && gm_lines_carrier(&bare).is_empty());
        assert_eq!(script_type_in(&bare), None);
    }

    /// Which hulls a bubble does not hold is the bundle's attribute, never a
    /// list of ids here (invented rows and groups).
    #[test]
    fn the_hulls_no_bubble_holds_are_read_by_the_immune_attribute() {
        use eve_server::content::Content;
        let rows = format!(
            "{}type\t1000811\t1000891\t22\t1\t1\t10.0\t0.0\t0.0\t1.0\t0\t\t{ATTR_IMMUNE}=1.0\tInvented Super\n\
             type\t1000812\t1000892\t22\t1\t1\t10.0\t0.0\t0.0\t1.0\t0\t\t{ATTR_IMMUNE}=0.0\tInvented Frigate\n\
             type\t1000813\t1000892\t22\t1\t1\t10.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Plain\n",
            eve_server::typedata::FIXTURE
        );
        let mut c = Content::fixture();
        let table = eve_server::typedata::Table::parse(&rows).expect("rows parse");
        c.insert("typedata", eve_server::typedata::Installed::build(table));
        let set = immune_types_in(&c);
        assert!(set.contains(&1_000_811));
        assert!(!set.contains(&1_000_812), "zero is not immune");
        assert!(!set.contains(&1_000_813), "absent is not immune");
    }

    /// The probe path says why it does or does not launch, and a probe is
    /// one `[camp]` line per launch, whatever the turn count.
    #[test]
    fn the_probe_decision_names_its_reason() {
        let t = Instant::now();
        let prey = ball(9, 5_000.0);
        let why = |m: &Memory, o: &Observation| launch_verdict(m, o, &prey).map(|v| v.err());
        let (m, o) = setup(t);
        assert_eq!(why(&m, &o), Some(None), "ready");
        assert!(launch_verdict(&m, &o, &ball(9, LAUNCH_REACH * 2.0)).is_none());
        let (m, mut o) = setup(t);
        o.kit.launcher = Some(launcher(0));
        assert_eq!(why(&m, &o), Some(Some("launcher empty")));
        o.kit.launcher = None;
        assert_eq!(why(&m, &o), Some(Some("no launcher")));
        let (mut m, o) = setup(t);
        m.camp.blocked(t);
        assert_eq!(why(&m, &o), Some(Some("resting after a refusal")));
        let (mut m, o) = setup(t);
        m.camp.launched_at = Some(t);
        assert_eq!(why(&m, &o), Some(Some("own probe still up")));
        let (m, mut o) = setup(t);
        o.warp_bubble.zones = vec![([5_000.0, 0.0, 0.0], 10_000.0)];
        assert_eq!(why(&m, &o), Some(Some("prey already held")));
        let (m, mut o) = setup(t);
        o.park.as_mut().unwrap().ego_warping = true;
        assert_eq!(why(&m, &o), Some(Some("warping")));
        // The log: a state prints once; a new launch prints again.
        let (mut m, o) = setup(t);
        note_launch(&mut m, &o, &prey);
        assert_eq!(m.camp.launch_go, Some(0));
        let seq = m.camp.note_seq;
        note_launch(&mut m, &o, &prey);
        assert_eq!(m.camp.note_seq, seq, "the same decision: no second line");
        m.camp.launches = 1;
        m.camp.launched_at = Some(t);
        note_launch(&mut m, &o, &prey);
        assert_eq!(m.camp.launch_note, Some("own probe still up"));
        m.camp.launched_at = None;
        note_launch(&mut m, &o, &prey);
        assert_eq!(
            m.camp.launch_go,
            Some(1),
            "the second probe is its own line"
        );
    }

    /// `hold_position` drops the moves and nothing else: the launch stays.
    #[test]
    fn holding_the_spot_keeps_the_probe_launch() {
        let t = Instant::now();
        let (_, o) = setup(t);
        let l = o.kit.launcher.unwrap();
        let kept = hold_position(vec![
            Act::Orbit { id: 9, range: 1.0 },
            Act::Kit(KitAct::Launch(l)),
            Act::WarpToItem { id: 3, range: 1.0 },
            Act::Lock(9),
        ]);
        assert_eq!(kept, vec![Act::Kit(KitAct::Launch(l)), Act::Lock(9)]);
    }

    const IMMUNE_HULL: i64 = 1_000_811;
    const SLOT: i64 = 19;

    fn hostile(id: i64, x: f64, hull: Option<i64>) -> Pilot {
        Pilot {
            ball: ball(id, x),
            hostile: true,
            type_id: hull,
            ..friend(id)
        }
    }

    /// A HIC in a null system: a field, a script slot with `script_loaded`,
    /// a script in the hold, a friend and one hostile at 3 km (a hull a bubble
    /// holds, or [`IMMUNE_HULL`]) locked.
    fn hic(now: Instant, immune_prey: bool, script_loaded: bool) -> (Memory, Observation, Ball) {
        let (mut m, mut o) = setup(now);
        o.kit.launcher = None;
        o.kit.field = Some(field());
        o.kit.script = Some(crate::pvp_kit::FieldScript {
            flag_id: SLOT,
            loaded: script_loaded.then_some(SCRIPT_TYPE),
        });
        m.camp.script_type = Some(SCRIPT_TYPE);
        m.camp.immune.insert(IMMUNE_HULL);
        o.cargo = vec![script_row(SCRIPT_ITEM)];
        let prey = ball(9, 3_000.0);
        let hull = immune_prey.then_some(IMMUNE_HULL);
        o.hunt.pilots = vec![friend(20), hostile(9, 3_000.0, hull)];
        o.locked = vec![9];
        (m, o, prey)
    }

    fn running(o: &mut Observation) {
        o.kit.field = Some(KitModule {
            active: true,
            ..field()
        });
    }

    #[test]
    fn a_bubbleable_prey_gets_the_field() {
        let (mut m, o, prey) = hic(Instant::now(), false, false);
        let acts = generator_acts(&mut m, &o, &prey);
        assert!(
            matches!(acts[..], [Act::Kit(KitAct::FieldOn(_))]),
            "{acts:?}"
        );
        // A lone HIC is a feed: no friend, no field.
        let (mut m, mut o, prey) = hic(Instant::now(), false, false);
        o.hunt.pilots.retain(|p| p.hostile);
        assert!(generator_acts(&mut m, &o, &prey).is_empty());
    }

    #[test]
    fn a_ship_no_bubble_holds_gets_the_script_and_then_the_point() {
        let t = Instant::now();
        // 1. No script in the generator, one in the hold: load it.
        let (mut m, o, prey) = hic(t, true, false);
        let Some(Act::LoadAmmo {
            guns,
            charges,
            location,
        }) = generator_acts(&mut m, &o, &prey).into_iter().next()
        else {
            panic!("no load");
        };
        assert_eq!(guns, vec![(field().item_id, SLOT)]);
        assert_eq!(charges, vec![SCRIPT_ITEM]);
        assert_eq!(location, SHIP);
        assert_eq!(m.camp.focus_note, Some("load"));
        // The load is locked out for a moment, not repeated, and no field is
        // switched on only to be stopped for the load.
        assert!(generator_acts(&mut m, &o, &prey).is_empty());
        // 2. Loaded, the prey locked: aim the point at it.
        let (mut m, o, prey) = hic(t, true, true);
        let acts = generator_acts(&mut m, &o, &prey);
        assert!(
            matches!(acts[..], [Act::Kit(KitAct::FieldPoint(f, 9))] if f.item_id == field().item_id),
            "{acts:?}"
        );
        assert_eq!(m.camp.focus_note, Some("point"));
        // The bubble is never run with a script in it.
        assert_eq!(field_now(&m, &o, &prey), None);
        // Not locked yet: nothing (the fight's own acts lock it).
        let (mut m, mut o, prey) = hic(t, true, true);
        o.locked.clear();
        assert!(generator_acts(&mut m, &o, &prey).is_empty());
        // Out of the point's reach: nothing.
        let (mut m, mut o, _) = hic(t, true, true);
        m.camp.immune_at = Some(t); // seen lately: the script is held
        o.hunt.pilots = vec![friend(20), hostile(9, 100_000.0, Some(IMMUNE_HULL))];
        assert!(generator_acts(&mut m, &o, &ball(9, 100_000.0)).is_empty());
    }

    /// A running generator is stopped before its script is loaded or unloaded.
    #[test]
    fn a_running_generator_is_stopped_before_its_script_changes() {
        let t = Instant::now();
        let (mut m, mut o, prey) = hic(t, true, false);
        running(&mut o);
        let acts = generator_acts(&mut m, &o, &prey);
        assert!(
            matches!(acts[..], [Act::Kit(KitAct::FieldOff(_))]),
            "{acts:?}"
        );
        let (mut m, mut o, prey) = hic(t, false, true);
        running(&mut o);
        let acts = generator_acts(&mut m, &o, &prey);
        assert!(
            matches!(acts[..], [Act::Kit(KitAct::FieldOff(_))]),
            "{acts:?}"
        );
        // Idle: the script comes out, to the hold.
        let (mut m, o, prey) = hic(t, false, true);
        let acts = generator_acts(&mut m, &o, &prey);
        assert!(
            matches!(acts[..], [Act::Kit(KitAct::ScriptOut(_))]),
            "{acts:?}"
        );
        assert_eq!(m.camp.focus_note, Some("out"));
    }

    /// The point's target is gone: the idle generator gives the script up.
    #[test]
    fn the_script_comes_out_when_the_point_target_is_gone() {
        let t = Instant::now();
        let (mut m, mut o, _) = hic(t, true, true);
        o.hunt.pilots = vec![friend(20)];
        let acts = quiet(&mut m, &o);
        assert!(
            matches!(acts[..], [Act::Kit(KitAct::ScriptOut(_))]),
            "{acts:?}"
        );
        assert_eq!(m.camp.focus_note, Some("gone"));
        // Running: stopped first.
        let (mut m, mut o, _) = hic(t, true, true);
        o.hunt.pilots = vec![friend(20)];
        running(&mut o);
        let acts = quiet(&mut m, &o);
        assert!(
            matches!(acts[..], [Act::Kit(KitAct::FieldOff(_))]),
            "{acts:?}"
        );
        // No script loaded: nothing to take out.
        let (mut m, o, _) = hic(t, true, false);
        assert!(quiet(&mut m, &o).is_empty());
    }

    /// Without a script the HIC says why, once, and keeps the field.
    #[test]
    fn without_a_script_the_hic_says_why_and_keeps_the_field() {
        let t = Instant::now();
        let (mut m, mut o, prey) = hic(t, true, false);
        o.cargo.clear();
        let acts = generator_acts(&mut m, &o, &prey);
        assert!(
            matches!(acts[..], [Act::Kit(KitAct::FieldOn(_))]),
            "{acts:?}"
        );
        assert_eq!(m.camp.focus_note, Some("none"));
        let seq = m.camp.note_seq;
        generator_acts(&mut m, &o, &prey);
        assert_eq!(m.camp.note_seq, seq, "said once");
        // The bundle names no script at all: the same, with its own reason.
        let (mut m, o, prey) = hic(t, true, false);
        m.camp.script_type = None;
        let acts = generator_acts(&mut m, &o, &prey);
        assert!(
            matches!(acts[..], [Act::Kit(KitAct::FieldOn(_))]),
            "{acts:?}"
        );
        // A hull the grid does not name is bubbleable.
        let (mut m, mut o, prey) = hic(t, true, false);
        o.hunt.pilots = vec![friend(20), hostile(9, 3_000.0, None)];
        assert!(!prey_immune(&m, &o, &prey));
        assert!(matches!(
            generator_acts(&mut m, &o, &prey)[..],
            [Act::Kit(KitAct::FieldOn(_))]
        ));
    }

    /// The whole camp turn (`acts`) carries the point.
    #[test]
    fn the_camp_turn_aims_the_point_at_a_ship_no_bubble_holds() {
        let t = Instant::now();
        let (mut m, o, prey) = hic(t, true, true);
        let acts = acts(&mut m, &o, &prey);
        assert!(
            acts.iter()
                .any(|a| matches!(a, Act::Kit(KitAct::FieldPoint(_, 9)))),
            "{acts:?}"
        );
    }

    const ESCORT: i64 = 11;

    /// A super and its escort at like ranges: the ship no bubble holds is the
    /// generator's target whichever of the two is nearest, so the script is
    /// neither unloaded nor reloaded as the nearest alternates, and the bot
    /// locks the super itself when the fight (which locks only the nearest)
    /// does not.
    #[test]
    fn the_nearest_prey_alternating_does_not_swap_script_and_field() {
        let t = Instant::now();
        for (super_x, escort_x) in [(3_000.0, 2_000.0), (2_000.0, 3_000.0)] {
            let escort_nearest = escort_x < super_x;
            let (mut m, mut o, _) = hic(t, true, true);
            o.hunt.pilots = vec![
                friend(20),
                hostile(9, super_x, Some(IMMUNE_HULL)),
                hostile(ESCORT, escort_x, None),
            ];
            o.lock_range = 50_000.0;
            // Nothing is locked yet; the fight's prey is the nearest.
            o.locked.clear();
            let nearest = if escort_nearest {
                ball(ESCORT, escort_x)
            } else {
                ball(9, super_x)
            };
            let acts_now = acts(&mut m, &o, &nearest);
            assert!(
                !acts_now.iter().any(|a| matches!(
                    a,
                    Act::Kit(KitAct::ScriptOut(_) | KitAct::FieldOn(_) | KitAct::FieldPoint(_, _))
                )),
                "no swap, and no point before the lock: {acts_now:?}"
            );
            // The escort is nearer: the super is locked here. The super
            // nearest: the fight locks it, so this adds no second lock.
            assert_eq!(
                acts_now.contains(&Act::Lock(9)),
                escort_nearest,
                "{acts_now:?}"
            );
            // A lock pending is not asked again.
            o.pending = vec![9];
            assert!(!acts(&mut m, &o, &nearest).contains(&Act::Lock(9)));
            o.pending.clear();
            // The lock lands: the point goes on the super, never the escort.
            o.locked = vec![9];
            let acts_locked = acts(&mut m, &o, &nearest);
            assert!(
                acts_locked
                    .iter()
                    .any(|a| matches!(a, Act::Kit(KitAct::FieldPoint(_, 9)))),
                "the point goes on the super: {acts_locked:?}"
            );
            assert!(
                !acts_locked.iter().any(|a| matches!(
                    a,
                    Act::Kit(
                        KitAct::ScriptOut(_) | KitAct::FieldOn(_) | KitAct::FieldPoint(_, ESCORT)
                    )
                )),
                "no swap: {acts_locked:?}"
            );
        }
        // Out of lock range: no lock is asked.
        let (mut m, mut o, _) = hic(t, true, true);
        o.hunt.pilots = vec![
            friend(20),
            hostile(9, 3_000.0, Some(IMMUNE_HULL)),
            hostile(ESCORT, 2_000.0, None),
        ];
        o.locked.clear();
        o.lock_range = 1_000.0;
        assert!(!acts(&mut m, &o, &ball(ESCORT, 2_000.0)).contains(&Act::Lock(9)));
        // Unloaded: the same two ships load the script (and lock the super),
        // never run the field.
        let (mut m, mut o, _) = hic(t, true, false);
        o.hunt.pilots = vec![
            friend(20),
            hostile(9, 3_000.0, Some(IMMUNE_HULL)),
            hostile(ESCORT, 2_000.0, None),
        ];
        o.locked.clear();
        o.lock_range = 50_000.0;
        let acts = acts(&mut m, &o, &ball(ESCORT, 2_000.0));
        assert!(
            matches!(acts[..], [Act::LoadAmmo { .. }, Act::Lock(9)]),
            "{acts:?}"
        );
    }

    /// The script stays loaded for [`SCRIPT_HOLD`] after the last ship no
    /// bubble holds left reach; meanwhile the generator points the current
    /// prey (a point holds any ship); then the script comes out and the
    /// field runs.
    #[test]
    fn the_script_is_held_and_points_the_prey_until_the_hold_is_over() {
        let t = Instant::now();
        let (mut m, o, prey) = hic(t, true, true);
        assert!(!acts(&mut m, &o, &prey).is_empty());
        assert_eq!(m.camp.immune_at, Some(t));
        // Only the escort is left, locked: held, and pointed.
        let (_, mut o2, _) = hic(t, false, true);
        o2.now = t + SCRIPT_HOLD - Duration::from_secs(1);
        let escort = ball(9, 3_000.0);
        let acts_held = generator_acts(&mut m, &o2, &escort);
        assert!(
            matches!(acts_held[..], [Act::Kit(KitAct::FieldPoint(_, 9))]),
            "{acts_held:?}"
        );
        // Not locked yet: nothing (the fight locks the nearest).
        o2.locked.clear();
        assert!(generator_acts(&mut m, &o2, &escort).is_empty());
        assert!(quiet(&mut m, &o2).is_empty(), "held when quiet too");
        // The hold is over: the script comes out.
        o2.locked = vec![9];
        o2.now = t + SCRIPT_HOLD;
        let acts_out = generator_acts(&mut m, &o2, &escort);
        assert!(
            matches!(acts_out[..], [Act::Kit(KitAct::ScriptOut(_))]),
            "{acts_out:?}"
        );
    }

    /// A running field with a hostile of char 14 inside it since `t`.
    fn inside_field(t: Instant) -> (Memory, Observation) {
        let (mut m, mut o, _) = hic(t, false, false);
        running(&mut o);
        o.hunt.pilots = vec![friend(20), hostile(9, 3_000.0, None)];
        learn(&mut m, &o);
        assert!(m.camp.inside.contains_key(&14), "inside since t");
        (m, o)
    }

    /// Turns the pilot away and runs the wait out; `Some(learned?)`.
    fn leave_and_wait(m: &mut Memory, o: &mut Observation, at: Instant) -> bool {
        o.now = at;
        o.hunt.pilots = vec![friend(20)];
        learn(m, o);
        for _ in 0..LEARN_TURNS {
            learn(m, o);
        }
        m.camp.learned.contains(&14)
    }

    /// A ship that sits in the bot's own running field and warps out is
    /// immune for this camp (a nullifier fit the hull type does not show) ...
    #[test]
    fn a_ship_that_warps_out_of_the_running_field_is_learned_immune() {
        let t = Instant::now();
        let (mut m, mut o) = inside_field(t);
        let prey = ball(9, 3_000.0);
        assert!(!prey_immune(&m, &o, &prey));
        // The decision waits for the wreck check: not learned on the turn.
        o.now = t + LEARN_AFTER;
        o.hunt.pilots = vec![friend(20)];
        learn(&mut m, &o);
        assert!(m.camp.learned.is_empty(), "delayed by a turn");
        assert_eq!(m.camp.vanished.len(), 1);
        for _ in 0..LEARN_TURNS {
            learn(&mut m, &o);
        }
        assert!(m.camp.learned.contains(&14));
        // The same pilot back is immune, whatever the hull says.
        o.hunt.pilots = vec![friend(20), hostile(9, 3_000.0, None)];
        assert!(prey_immune(&m, &o, &prey));
    }

    /// ... but not when he left in another way.
    #[test]
    fn a_ship_that_left_in_another_way_is_not_learned() {
        let t = Instant::now();
        let later = t + LEARN_AFTER;
        // Gone too soon.
        let (mut m, mut o) = inside_field(t);
        assert!(!leave_and_wait(&mut m, &mut o, t + Duration::from_secs(1)));
        // A wreck there when he vanished.
        let (mut m, mut o) = inside_field(t);
        o.hunt.wrecks = vec![ball(77, 3_000.0)];
        assert!(!leave_and_wait(&mut m, &mut o, later));
        // A wreck that shows a turn after he vanished (a late kill).
        let (mut m, mut o) = inside_field(t);
        o.now = later;
        o.hunt.pilots = vec![friend(20)];
        learn(&mut m, &o);
        o.hunt.wrecks = vec![ball(77, 3_000.0)];
        for _ in 0..LEARN_TURNS {
            learn(&mut m, &o);
        }
        assert!(m.camp.learned.is_empty(), "a late wreck");
        // A gate there: he jumped.
        let (mut m, mut o) = inside_field(t);
        o.warp_bubble.gates = vec![ball(GATE, 3_000.0)];
        assert!(!leave_and_wait(&mut m, &mut o, later));
        // A station there: he docked.
        let (mut m, mut o) = inside_field(t);
        o.park.as_mut().unwrap().station = Some(ball(7, 3_000.0));
        assert!(!leave_and_wait(&mut m, &mut o, later));
        // The pilot list is empty: a resync, nobody left.
        let (mut m, mut o) = inside_field(t);
        o.now = later;
        o.hunt.pilots.clear();
        learn(&mut m, &o);
        for _ in 0..LEARN_TURNS {
            learn(&mut m, &o);
        }
        assert!(
            m.camp.learned.is_empty() && m.camp.inside.is_empty(),
            "resync"
        );
        // The field was stopped the turn he vanished.
        let (mut m, mut o) = inside_field(t);
        o.kit.field = Some(field());
        assert!(!leave_and_wait(&mut m, &mut o, later));
        // A stopped field learns nothing from the start.
        let (mut m, mut o, _) = hic(t, false, false);
        o.hunt.pilots = vec![friend(20), hostile(9, 3_000.0, None)];
        learn(&mut m, &o);
        assert!(m.camp.inside.is_empty());
    }

    /// What the camp learned goes when the camp order ends.
    #[test]
    fn what_the_camp_learned_goes_with_the_camp() {
        let t = Instant::now();
        let (mut m, mut o) = inside_field(t);
        assert!(leave_and_wait(&mut m, &mut o, t + LEARN_AFTER));
        m.camp.vanished.push(Vanished {
            char_id: 1,
            at: [0.0; 3],
            turns: 1,
        });
        m.camp.immune_at = Some(t);
        camp_over(&mut m);
        assert!(m.camp.learned.is_empty() && m.camp.inside.is_empty());
        assert!(m.camp.vanished.is_empty() && m.camp.immune_at.is_none());
    }

    /// The script put in the hold is the bundle's type, not any of group 908.
    #[test]
    fn only_the_named_script_type_goes_in_the_hold() {
        let scram = ItemRow {
            type_id: SCRIPT_TYPE + 1,
            ..row(SCRIPT_ITEM - 1, GROUP_SCRIPT)
        };
        let rows = vec![scram, script_row(SCRIPT_ITEM), row(SCRIPT_ITEM + 1, 83)];
        assert_eq!(
            pick_script(&rows, Some(SCRIPT_TYPE)).map(|r| r.item_id),
            Some(SCRIPT_ITEM),
            "the lower-numbered scrambling script is passed over"
        );
        assert_eq!(pick_script(&rows[..1], Some(SCRIPT_TYPE)), None);
        assert_eq!(pick_script(&rows, None), None, "no type named, none picked");
    }

    /// Only one act on the generator a turn: a stop sent for the prey out of
    /// reach is not followed by a second one for the script.
    #[test]
    fn one_stop_a_turn() {
        let t = Instant::now();
        let (mut m, mut o, _) = hic(t, true, false);
        running(&mut o);
        o.hunt.pilots = vec![friend(20), hostile(9, 100_000.0, Some(IMMUNE_HULL))];
        let acts = acts(&mut m, &o, &ball(9, 100_000.0));
        let stops = acts
            .iter()
            .filter(|a| matches!(a, Act::Kit(KitAct::FieldOff(_))))
            .count();
        assert_eq!(stops, 1, "{acts:?}");
    }

    /// Only a camper runs the generator logic: another role's turn adds none.
    #[test]
    fn only_a_camper_runs_the_generator_logic() {
        let t = Instant::now();
        let (mut m, o, prey) = hic(t, true, true);
        m.combat.role = Some(Role::Dps);
        assert!(generator_acts(&mut m, &o, &prey).is_empty());
        assert_eq!(immune_target(&mut m, &o), None);
        assert_eq!(m.camp.immune_at, None);
    }

    /// A lock is asked only while there is a slot for it.
    #[test]
    fn the_target_is_locked_only_under_the_lock_slots() {
        let t = Instant::now();
        let (_, mut o, _) = hic(t, true, true);
        o.lock_range = 50_000.0;
        o.locked.clear();
        let target = ball(9, 3_000.0);
        assert_eq!(lock_act(&o, &target), Some(Act::Lock(9)));
        o.lock_max = 2;
        o.locked = vec![30, 31];
        assert_eq!(lock_act(&o, &target), None, "both slots taken");
        o.locked = vec![30];
        o.pending = vec![];
        assert_eq!(lock_act(&o, &target), Some(Act::Lock(9)));
        o.pending = vec![31];
        assert_eq!(lock_act(&o, &target), None, "one locked, one pending");
        // No limit read yet: the default stands.
        o.lock_max = 0;
        o.pending.clear();
        o.locked = (30..30 + crate::ewar_aim::LOCK_MAX_DEFAULT as i64).collect();
        assert_eq!(lock_act(&o, &target), None);
    }

    /// The camp's learning goes on the change from the camp to another order
    /// only.
    #[test]
    fn the_camps_memory_goes_on_the_change_from_the_camp_only() {
        let t = Instant::now();
        let (mut m, _, _) = hic(t, true, true);
        m.camp.learned.insert(14);
        m.camp.immune_at = Some(t);
        order_turn(&mut m, false); // a roaming gang's fight: never a camp
        assert!(m.camp.immune_at.is_some() && !m.camp.learned.is_empty());
        order_turn(&mut m, true);
        order_turn(&mut m, true);
        assert!(m.camp.immune_at.is_some());
        order_turn(&mut m, false); // the camp ends
        assert!(m.camp.learned.is_empty() && m.camp.immune_at.is_none());
        assert!(!m.camp.was_camp);
    }

    /// The turret slots cap an interdictor's guns; a HIC with its field and
    /// no guns to be had is released from them.
    #[test]
    fn an_interdictor_asks_for_no_more_guns_than_the_hull_has_slots() {
        const HULL: i64 = 1_000_790;
        let t = Instant::now();
        let (mut m, mut o) = setup(t);
        m.policy.pvp = Some(Arc::new(
            Ground::new().with_security(SYSTEM, -0.5).with_fit(
                HULL,
                crate::counter::BUBBLE_GROUPS[1],
                0,
                2,
            ),
        ));
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(HULL),
        });
        assert_eq!(guns_wanted(&m, Some(HULL), 3), 2);
        assert_eq!(guns_wanted(&m, Some(HULL), 1), 1);
        assert_eq!(guns_wanted(&m, Some(HULL + 1), 3), 3, "an unlisted hull");
        m.combat.role = Some(Role::Dps);
        assert_eq!(guns_wanted(&m, Some(HULL), 3), 3, "not an interdictor");
        m.combat.role = Some(Role::Interdict);
        // Field fitted, no gun in the hangar: released. With a gun: wanted,
        // until a fit is refused.
        o.kit.field = Some(field());
        assert!(gunless_hic(&m, &o, false));
        assert!(!gunless_hic(&m, &o, true));
        // A refusal counts only after the guns were tried.
        o.failures.insert(crate::brain::ActKind::FitAuto, 1);
        assert!(!gunless_hic(&m, &o, true), "another module's refusal");
        m.camp.gun_fit_tried = true;
        assert!(gunless_hic(&m, &o, true));
        // No fitted field (a hangar one is not enough): not released.
        o.hangar = vec![ItemRow {
            item_id: 5,
            ..row(5, 1_000_899)
        }];
        m.camp.field_group = Some(1_000_899);
        o.kit.field = None;
        assert!(!gunless_hic(&m, &o, false));
        assert_eq!(field_to_fit(&m, &o).map(|r| r.item_id), Some(5));
        m.hunt.tackle_tried.push(5);
        assert_eq!(field_to_fit(&m, &o), None, "tried once");
    }

    /// The HIC sits where its field holds the whole landing zone of the gate
    /// (the log: the HIC at 10 km, the client 17143 m from it, radius 16000).
    #[test]
    fn a_hic_sits_where_its_field_holds_the_gates_landing_zone() {
        let range = 16_000.0;
        for gate_radius in [500.0, 2_000.0, 3_000.0] {
            let s = hic_standoff(range, gate_radius);
            // The far side of the zone, seen from the HIC across the gate.
            let far = s + gate_radius + LANDING_ZONE;
            assert!(far <= range - FIELD_MARGIN, "radius {gate_radius}: {far}");
            assert!(s <= GATE_STANDOFF, "never farther than a dictor");
            assert!(
                s >= gate_radius + eve_server::space_state::WARP_STANDOFF,
                "never nearer than the server lands a warp"
            );
        }
        // A bigger field lets it sit farther, up to the dictor's standoff.
        assert_eq!(hic_standoff(100_000.0, 2_000.0), GATE_STANDOFF);
        // A field too small for the gate: the nearest it can sit.
        assert_eq!(
            hic_standoff(5_000.0, 3_000.0),
            3_000.0 + eve_server::space_state::WARP_STANDOFF
        );
    }

    /// `post` asks the HIC's standoff for the gate, a dictor's the old one.
    #[test]
    fn a_camping_hic_warps_in_at_its_field_standoff() {
        use crate::travel::Map;
        const HIC_HULL: i64 = 1_000_780;
        let t = Instant::now();
        let (mut m, mut o) = setup(t);
        m.policy.pvp = Some(Arc::new(
            Ground::new().with_security(SYSTEM, -0.5).with_fit(
                HIC_HULL,
                crate::counter::BUBBLE_GROUPS[1],
                0,
                0,
            ),
        ));
        m.policy.travel = Some(Arc::new(
            Map::new().with_gate(SYSTEM, GATE, 1_000_098, 2_000_504),
        ));
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(HIC_HULL),
        });
        o.kit.launcher = None;
        // The field of the log: 16 km.
        o.kit.field = Some(KitModule {
            range: 16_000.0,
            ..field()
        });
        let mut gate = ball(GATE, 1.0e7);
        gate.radius = 2_000.0;
        o.warp_bubble.gates = vec![gate];
        let park = o.park.clone().unwrap();
        let want = hic_standoff(16_000.0, gate.radius);
        assert_ne!(want, GATE_STANDOFF, "the field's size matters here");
        assert_eq!(
            post(&mut m, &o, &park, SYSTEM),
            Some(Act::WarpToItem {
                id: GATE,
                range: want
            })
        );
        // A dictor (no field generator) asks the old standoff.
        let (mut d, mut od) = setup(t);
        d.policy.travel = m.policy.travel.clone();
        od.warp_bubble.gates = vec![gate];
        let pd = od.park.clone().unwrap();
        assert_eq!(
            post(&mut d, &od, &pd, SYSTEM),
            Some(Act::WarpToItem {
                id: GATE,
                range: d.camp.standoff()
            })
        );
    }

    /// The log of regress-w32b: the carrier `RedAnchors1x1` was granted its
    /// unit at +0 s and moved it to the hold of the rookie ship; the Rifter
    /// boarded at +10 s did not have it (`unit=none in hold`). The grant and
    /// the move wait for the combat hull, the undock waits for the unit, and
    /// the sale and the hold keep it for this bot.
    #[test]
    fn the_carrier_takes_its_unit_only_once_it_flies_its_combat_hull() {
        const ROOKIE: i64 = 1_000_795;
        let t = Instant::now();
        let (mut m, mut o) = setup(t);
        m.combat.role = None;
        o.kit.launcher = None;
        m.policy.dev = Some(Arc::new(crate::dev::DevBot {
            group: "invented".to_owned(),
            ticker: "INV".to_owned(),
            skills: crate::dev::Skills::All5,
            skill_ids: None,
            hull: None,
            home: None,
            boss: false,
            hall: Arc::default(),
        }));
        m.name = format!("Red{CARRIER_STEM}1x1");
        assert!(is_carrier(&m) && needs_kit(&m, &o), "the bot of the log");
        // Docked in the rookie ship: not yet.
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(ROOKIE),
        });
        assert!(!carrier_may_take_unit(&m, &o));
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: None,
        });
        assert!(!carrier_may_take_unit(&m, &o), "the hull not yet read");
        // The Rifter boarded: now, and it waits for the unit in its hold.
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(crate::escort::RIFTER),
        });
        assert!(carrier_may_take_unit(&m, &o));
        assert!(kit_pending(&mut m, &o), "the undock waits for the unit");
        // regress-w32e: its kind's hull is no entry of the combat table (the
        // Stabber), and it must take the unit all the same.
        const KIND_HULL: i64 = 1_000_797;
        assert!(!crate::hull_choice::is_combat_hull(KIND_HULL));
        m.policy.dev = Some(Arc::new(crate::dev::DevBot {
            group: "invented".to_owned(),
            ticker: "INV".to_owned(),
            skills: crate::dev::Skills::All5,
            skill_ids: None,
            hull: Some(KIND_HULL),
            home: None,
            boss: false,
            hall: Arc::default(),
        }));
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(KIND_HULL),
        });
        assert!(carrier_may_take_unit(&m, &o), "the kind's own hull");
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(ROOKIE),
        });
        assert!(!carrier_may_take_unit(&m, &o), "still not the rookie ship");
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(crate::escort::RIFTER),
        });
        o.cargo = vec![row(UNIT_ITEM, GROUP_UNIT)];
        assert!(!kit_pending(&mut m, &o));
        // Without a dev bot it is no carrier at all.
        m.policy.dev = None;
        assert!(!is_carrier(&m) && !needs_kit(&m, &o));
    }

    /// The hold of the carrier's hull must take the unit: a hull with a small
    /// hold refused it (the Add was a note nobody read). Invented sizes.
    #[test]
    fn the_carriers_hold_must_take_the_unit() {
        use eve_server::content::Content;
        let row = |id: i64, volume: f64, capacity: f64, name: &str| {
            format!("type\t{id}\t1000893\t6\t1\t1\t{volume}\t{capacity}\t0.0\t1.0\t0\t\t\t{name}\n")
        };
        let rows = format!(
            "{}{}{}{}",
            eve_server::typedata::FIXTURE,
            row(1_000_821, 150.0, 0.0, UNIT_NAME),
            row(1_000_822, 5_000.0, 100.0, "Invented Small Hold"),
            row(1_000_823, 5_000.0, 400.0, "Invented Big Hold"),
        );
        let mut c = Content::fixture();
        let table = eve_server::typedata::Table::parse(&rows).expect("rows parse");
        c.insert("typedata", eve_server::typedata::Installed::build(table));
        let (cap, vol) = hold_for_unit(&c, 1_000_822).expect("both rows");
        assert!(cap < vol, "the small hold cannot take it: {cap} < {vol}");
        let (cap, vol) = hold_for_unit(&c, 1_000_823).expect("both rows");
        assert!(cap >= vol, "the big one can: {cap} >= {vol}");
        assert_eq!(hold_for_unit(&c, 1_000_999), None, "an unlisted hull");
        assert_eq!(hold_for_unit(&Content::fixture(), 1_000_823), None);
    }

    /// The log of regress-w32c: the HIC asked its warp with the gate off its
    /// grid (the dictor's 10 km), landed at 10 km, and the next turns left it
    /// there. With the gate on the grid it closes in to its field's standoff,
    /// remembers it, and a later warp asks it.
    #[test]
    fn a_hic_that_landed_at_ten_km_closes_in_and_remembers_its_standoff() {
        use crate::travel::Map;
        const HIC_HULL: i64 = 1_000_781;
        let t = Instant::now();
        let (mut m, mut o) = setup(t);
        m.policy.pvp = Some(Arc::new(
            Ground::new().with_security(SYSTEM, -0.5).with_fit(
                HIC_HULL,
                crate::counter::BUBBLE_GROUPS[1],
                0,
                0,
            ),
        ));
        m.policy.travel = Some(Arc::new(
            Map::new().with_gate(SYSTEM, GATE, 1_000_098, 2_000_504),
        ));
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(HIC_HULL),
        });
        o.kit.launcher = None;
        o.kit.field = Some(KitModule {
            range: 16_000.0,
            ..field()
        });
        assert!(o.kit.field.unwrap().range > 0.0);
        let mut gate = ball(GATE, 0.0);
        gate.radius = 2_000.0;
        let park = o.park.clone().unwrap();
        // 1. Warping from the station: the gate is off the grid, the ask is
        // the default one.
        o.warp_bubble.gates.clear();
        assert_eq!(
            post(&mut m, &o, &park, SYSTEM),
            Some(Act::WarpToItem {
                id: GATE,
                range: m.camp.standoff()
            })
        );
        // 2. Landed at 10 km from the gate, now on the grid: the HIC moves in.
        o.warp_bubble.gates = vec![gate];
        o.park.as_mut().unwrap().ego = ball(SHIP, GATE_STANDOFF);
        let park = o.park.clone().unwrap();
        o.now = t + POST_EVERY * 2;
        let want = hic_standoff(16_000.0, gate.radius);
        assert!(
            want < GATE_STANDOFF - HIC_SLACK,
            "the field needs it closer"
        );
        assert_eq!(
            post(&mut m, &o, &park, SYSTEM),
            Some(Act::Follow {
                id: GATE,
                range: want
            })
        );
        assert_eq!(m.camp.hic_standoff, Some(want));
        // 3. In place: nothing more.
        o.park.as_mut().unwrap().ego = ball(SHIP, want);
        let park = o.park.clone().unwrap();
        o.now = t + POST_EVERY * 4;
        assert_eq!(post(&mut m, &o, &park, SYSTEM), None);
        // 4. A later warp from a grid without the gate asks the standoff it
        // learned; a dictor does not.
        o.warp_bubble.gates.clear();
        m.camp.posted = None;
        o.now = t + POST_EVERY * 6;
        let far = ParkView {
            ego: ball(SHIP, 5.0e7),
            ..park
        };
        assert_eq!(
            post(&mut m, &o, &far, SYSTEM),
            Some(Act::WarpToItem {
                id: GATE,
                range: want
            })
        );
    }

    /// regress-w32d: a HIC (a field, no launcher) logged `launch=refused: no
    /// launcher` every fight; a dictor with none still does.
    #[test]
    fn a_hic_says_nothing_about_a_launcher_it_does_not_have() {
        const HIC_HULL: i64 = 1_000_782;
        let t = Instant::now();
        let prey = ball(9, 5_000.0);
        let (mut m, mut o) = setup(t);
        o.kit.launcher = None;
        assert!(
            launch_verdict(&m, &o, &prey).is_some(),
            "a dictor with none"
        );
        m.policy.pvp = Some(Arc::new(
            Ground::new().with_security(SYSTEM, -0.5).with_fit(
                HIC_HULL,
                crate::counter::BUBBLE_GROUPS[1],
                0,
                0,
            ),
        ));
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(HIC_HULL),
        });
        assert!(launch_verdict(&m, &o, &prey).is_none());
    }

    /// The standoff decision is said once per change, with what it reads.
    #[test]
    fn the_standoff_decision_is_traced_on_change() {
        let t = Instant::now();
        let (mut m, mut o) = setup(t);
        o.warp_bubble.gates = vec![ball(GATE, 1.0e7)];
        let park = o.park.clone().unwrap();
        trace_standoff(&mut m, &o, &park, GATE, false, false);
        let first = m.camp.standoff_note.clone().expect("said");
        assert!(first.contains("gate_on_grid=true") && first.contains("hic=false"));
        let seq = m.camp.note_seq;
        trace_standoff(&mut m, &o, &park, GATE, false, false);
        assert_eq!(m.camp.note_seq, seq, "the same decision: no second line");
        trace_standoff(&mut m, &o, &park, GATE, true, true);
        assert_ne!(m.camp.standoff_note.as_deref(), Some(first.as_str()));
    }
}
