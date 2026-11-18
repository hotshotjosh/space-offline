//! The AIR new-player experience, run from the client's own node graphs.
//!
//! The server half of the tutorial is the `server`-tagged master graph and
//! its scene subgraphs (`docs/re/node-graphs.md`); this module runs it with
//! [`crate::node_graph::engine::Runtime`], one per character with an active
//! tutorial, and delivers what it asks for:
//!
//! - `OnStartClientNodeGraph(graph_id, instance, {dungeon_id, server_graph_instance_id, solar_system_id})`,
//!   `OnClientNodeGraphMessage(instance, key, value)`, `OnStopClientNodeGraph(instance)`
//!   **[V-TQ]** (a live server's pushes, recorded on a V24.02 client);
//! - `OnAirNpeCheckpointChanged(character_id, checkpoint)` for `Set Checkpoint`
//!   and `OnAirNpeStateChanged(character_id, 2)` for `Complete AIR NPE` **[V-TQ]**.
//!
//! **Client messages wait for the client.** The client starts a graph on a
//! tasklet and subscribes its message listeners when its nodes start, so a
//! message pushed right behind `OnStartClientNodeGraph` can reach a context
//! with no listener yet and be dropped. A live server's first scene message
//! went out only after the client graph's first `process_client_blackboard_request`
//! **[V-TQ]**; here client messages are held until that call (or
//! [`HOLD_LIMIT`] passes).
//!
//! State: the delta kind [`KIND`], `character -> [state, checkpoint]`, where
//! state is `AirNpeState` (1 active, 2 completed, 3 skipped) **[V]**
//! (`service/live.rs`). A tutorial is started with `/airnpe start [checkpoint]`.
//!
//! Server atoms this module does not perform yet are logged, once per run,
//! as `[airnpe] action <name> not done`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eve_marshal::PyValue;

use crate::content::{Boxed, Origin, Sources, TableSpec};
use crate::node_graph::data::Library;
use crate::node_graph::engine::{Effect, Kw, Runtime};
use crate::node_graph::value::Value;
use crate::service::{Registry, ServiceError};

/// The delta kind: `character -> [state, checkpoint]`.
pub const KIND: &str = "airnpe";

/// The master server graph of the AIR tutorial **[V-data]**: the one
/// `server`+`character` graph whose `Start Client Node Graph` nodes start the
/// `npe` client master, with one exclusive scene subgraph per scene.
pub const MASTER_GRAPH: i64 = 49;

/// `AirNpeState` **[V]** (`service/live.rs`).
pub const ACTIVE: i64 = 1;
pub const COMPLETED: i64 = 2;

/// How long client messages wait for the client graph's first call.
pub const HOLD_LIMIT: Duration = Duration::from_secs(8);

/// How long after the state query the runtime starts: the query is what
/// brings the client's `node_graph` service up **[I]** (`service/live.rs`
/// says state 1 "pulls in the node_graph service").
pub const START_DELAY: Duration = Duration::from_secs(2);

// -- content ---------------------------------------------------------------------

/// The node graphs of the bundle, if it has them.
#[derive(Default)]
pub struct Graphs(pub Option<Arc<Library>>);

pub const TABLE: TableSpec = TableSpec {
    name: "nodegraphs",
    load,
    fixture: || Box::new(Graphs::default()) as Boxed,
};

fn load(src: &Sources) -> Result<Boxed, String> {
    let Some(b) = src.bundle else {
        return Ok(Box::new(Graphs::default()));
    };
    let Some(graphs) = b.path("nodegraphs/graphs.tsv") else {
        return Ok(Box::new(Graphs::default()));
    };
    src.note(Origin::Bundle);
    let dir = graphs
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let lib = Library::load(&dir)?;
    eprintln!("[airnpe] {} node graphs loaded", lib.graphs.len());
    Ok(Box::new(Graphs(Some(Arc::new(lib)))))
}

fn library(reg: &Registry) -> Option<Arc<Library>> {
    reg.content().try_get::<Graphs>().and_then(|g| g.0.clone())
}

// -- state -------------------------------------------------------------------------

fn key(character_id: i64) -> Vec<String> {
    vec![character_id.to_string()]
}

/// `(state, checkpoint)` of a character, `None` with no row.
pub fn stored(reg: &Registry, character_id: i64) -> Option<(i64, Option<i64>)> {
    let row = reg.deltas().of(KIND).get(&key(character_id))?;
    let state = row.first()?.parse().ok()?;
    let checkpoint = row.get(1).and_then(|s| s.parse().ok());
    Some((state, checkpoint))
}

fn put(reg: &mut Registry, character_id: i64, state: i64, checkpoint: Option<i64>) {
    let ck = checkpoint.map(|c| c.to_string()).unwrap_or_default();
    reg.deltas_mut()
        .put(KIND, key(character_id), vec![state.to_string(), ck]);
    reg.mark_dirty();
}

/// One character's running tutorial.
struct Run {
    rt: Runtime,
    started: Instant,
    ready: bool,
    held: Vec<Effect>,
    told: BTreeSet<String>,
}

/// The extension slot: running tutorials, and those waiting to start.
#[derive(Default)]
pub struct Runs {
    runs: HashMap<i64, Run>,
    pending: HashMap<i64, Instant>,
}

pub const SLOT: crate::world_ext::SlotSpec = crate::world_ext::SlotSpec {
    name: "air_npe",
    make,
};

fn make(_: &crate::content::Content) -> crate::world_ext::Boxed {
    Box::new(Runs::default())
}

// -- the air_npe service's answers ----------------------------------------------

/// `air_npe.is_air_npe_enabled`.
pub fn enabled(reg: &Registry, actor: Option<i64>) -> bool {
    actor
        .and_then(|c| stored(reg, c))
        .is_some_and(|(s, _)| s == ACTIVE)
        && library(reg).is_some()
}

/// `air_npe.get_air_npe_state`: the scalar state, and the cue to start the
/// runtime for an active one.
pub fn state(reg: &mut Registry, actor: Option<i64>) -> PyValue {
    let Some(c) = actor else { return PyValue::None };
    match stored(reg, c) {
        Some((ACTIVE, _)) if library(reg).is_some() => {
            let runs = reg.ext_mut::<Runs>();
            if !runs.runs.contains_key(&c) {
                runs.pending
                    .insert(c, crate::game_clock::clock::wall() + START_DELAY);
            }
            PyValue::Int(ACTIVE)
        }
        Some((s, _)) if s == ACTIVE => PyValue::None,
        Some((s, _)) => PyValue::Int(s),
        None => PyValue::None,
    }
}

// -- the node_graph service --------------------------------------------------------

fn run_for<'a>(
    reg: &'a mut Registry,
    actor: Option<i64>,
    instance: &str,
) -> Option<(i64, &'a mut Run)> {
    let c = actor?;
    let run = reg.ext_mut::<Runs>().runs.get_mut(&c)?;
    (run.rt.instance_id() == instance).then_some((c, run))
}

/// `node_graph.process_client_blackboard_request(graph_id, instance, key)`.
pub fn blackboard_request(
    reg: &mut Registry,
    actor: Option<i64>,
    instance: &str,
    bb_key: &str,
) -> PyValue {
    let Some((c, run)) = run_for(reg, actor, instance) else {
        return PyValue::None;
    };
    let value = run.rt.blackboard(bb_key);
    let first = !run.ready;
    run.ready = true;
    eprintln!("[airnpe] char {c} <- blackboard request {bb_key} = {value}");
    if first {
        deliver(reg, c);
    }
    to_py(&value)
}

/// `node_graph.process_client_graph_message(graph_id, instance, key, value)`.
pub fn graph_message(
    reg: &mut Registry,
    actor: Option<i64>,
    instance: &str,
    msg_key: &str,
    value: Value,
) {
    let Some((c, run)) = run_for(reg, actor, instance) else {
        return;
    };
    eprintln!("[airnpe] char {c} <- {msg_key} {value}");
    run.ready = true;
    run.rt.message(msg_key, value);
    deliver(reg, c);
}

// -- driving ----------------------------------------------------------------------

/// Start (or restart) the tutorial of `c` now.
fn start_run(reg: &mut Registry, c: i64) {
    let Some(lib) = library(reg) else { return };
    let Some((_, checkpoint)) = stored(reg, c) else {
        return;
    };
    let (station, system) = reg
        .character(c)
        .map_or((None, None), |ch| (ch.station_id, ch.solar_system_id));
    let mut bb = Kw::new();
    bb.insert("character_id".into(), Value::Int(c));
    bb.insert(
        "birth_station_id".into(),
        station.map_or(Value::None, Value::Int),
    );
    bb.insert(
        "birth_system_id".into(),
        system.map_or(Value::None, Value::Int),
    );
    let checkpoints: BTreeMap<i64, i64> = checkpoint
        .map(|ck| (MASTER_GRAPH, ck))
        .into_iter()
        .collect();
    let seed = crate::game_clock::clock::wall_system()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
        ^ (c as u64);
    let now = clock(reg);
    let Some(rt) = Runtime::start(lib, MASTER_GRAPH, c, bb, checkpoints, now, seed) else {
        eprintln!("[airnpe] char {c}: no graph {MASTER_GRAPH} in the bundle");
        return;
    };
    eprintln!(
        "[airnpe] char {c}: started graph {MASTER_GRAPH} at checkpoint {checkpoint:?} as {}",
        rt.instance_id()
    );
    let runs = reg.ext_mut::<Runs>();
    if let Some(mut old) = runs.runs.remove(&c) {
        old.rt.stop();
    }
    runs.runs.insert(
        c,
        Run {
            rt,
            started: crate::game_clock::clock::wall(),
            ready: false,
            held: Vec::new(),
            told: BTreeSet::new(),
        },
    );
    deliver(reg, c);
}

/// Seconds on the runtime's clock.
fn clock(_reg: &Registry) -> f64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    crate::game_clock::clock::wall_since(*START.get_or_init(crate::game_clock::clock::wall))
        .as_secs_f64()
}

/// How many rounds of "perform the actions, resume their flows" one delivery
/// runs before it leaves the rest for the next tick (a runaway guard).
const MAX_ROUNDS: usize = 64;

/// Apply a run's effects: checkpoints and state now, pushes in order, client
/// messages held until the client is ready. Each server action is performed
/// and its flow resumed with what it returned ([`Runtime::resolve`]), which
/// can raise more effects, so this goes round until none is left.
fn deliver(reg: &mut Registry, c: i64) {
    for _ in 0..MAX_ROUNDS {
        let Some(run) = reg.ext_mut::<Runs>().runs.get_mut(&c) else {
            return;
        };
        let effects: Vec<Effect> = run.rt.effects.drain(..).collect();
        for line in run.rt.log.drain(..) {
            eprintln!("[airnpe] char {c}: {line}");
        }
        let instance = run.rt.instance_id().to_string();
        let release = run.ready || crate::game_clock::clock::wall_since(run.started) >= HOLD_LIMIT;
        let out = release_or_hold(&mut run.held, effects, release);
        let mut told_new = Vec::new();
        for e in &out {
            if let Effect::Action { atom, .. } = e
                && !HANDLED.contains(&atom.as_str())
                && run.told.insert(atom.clone())
            {
                told_new.push(atom.clone());
            }
        }
        let running = run.rt.is_running();
        let mut done = Vec::new();
        for e in out {
            if let Some(d) = apply(reg, c, &instance, e) {
                done.push(d);
            }
        }
        for a in told_new {
            eprintln!("[airnpe] char {c}: action {a} not done");
        }
        if !running {
            eprintln!("[airnpe] char {c}: graph {MASTER_GRAPH} stopped");
            reg.ext_mut::<Runs>().runs.remove(&c);
            return;
        }
        if done.is_empty() {
            return;
        }
        let objects = spawned_objects(reg, c);
        let Some(run) = reg.ext_mut::<Runs>().runs.get_mut(&c) else {
            return;
        };
        run.rt.dungeon_objects = objects;
        for (ticket, outputs) in done {
            run.rt.resolve(ticket, outputs);
        }
    }
}

/// What goes out now, in order: with `release`, everything held and then
/// `effects`; without, every effect but the client messages, which join
/// `held` in order.
fn release_or_hold(held: &mut Vec<Effect>, effects: Vec<Effect>, release: bool) -> Vec<Effect> {
    let mut out = Vec::new();
    if release {
        out.append(held);
    }
    for e in effects {
        match e {
            Effect::ClientMessage { .. } if !release => held.push(e),
            other => out.push(other),
        }
    }
    out
}

/// Push or record one effect; for a server action, perform it and return
/// its ticket and outputs (none when it is not performed: the flow goes on).
fn apply(reg: &mut Registry, c: i64, server_instance: &str, e: Effect) -> Option<(u64, Kw)> {
    match e {
        Effect::StartClientGraph { graph_id, instance } => {
            eprintln!("[airnpe] char {c} -> OnStartClientNodeGraph({graph_id}, {instance})");
            let bb = crate::pyutil::pydict(vec![
                ("dungeon_id", PyValue::None),
                ("server_graph_instance_id", str_py(server_instance)),
                ("solar_system_id", PyValue::None),
            ]);
            notify(
                reg,
                c,
                "OnStartClientNodeGraph",
                vec![PyValue::Int(graph_id), str_py(&instance), bb],
            );
        }
        Effect::StopClientGraph { instance } => {
            eprintln!("[airnpe] char {c} -> OnStopClientNodeGraph({instance})");
            notify(reg, c, "OnStopClientNodeGraph", vec![str_py(&instance)]);
        }
        Effect::ClientMessage {
            instance,
            key,
            value,
        } => {
            eprintln!("[airnpe] char {c} -> {key} {value}");
            notify(
                reg,
                c,
                "OnClientNodeGraphMessage",
                vec![str_py(&instance), str_py(&key), to_py(&value)],
            );
        }
        Effect::Checkpoint {
            graph_id,
            checkpoint,
        } => {
            eprintln!("[airnpe] char {c} checkpoint {graph_id}:{checkpoint}");
            if graph_id == MASTER_GRAPH {
                let state = stored(reg, c).map_or(ACTIVE, |(s, _)| s);
                put(reg, c, state, Some(checkpoint));
                notify(
                    reg,
                    c,
                    "OnAirNpeCheckpointChanged",
                    vec![PyValue::Int(c), PyValue::Int(checkpoint)],
                );
            }
        }
        Effect::Action { atom, args, ticket } => {
            if atom == "Complete AIR NPE" {
                eprintln!("[airnpe] char {c}: tutorial complete");
                let ck = stored(reg, c).and_then(|(_, k)| k);
                put(reg, c, COMPLETED, ck);
                notify(
                    reg,
                    c,
                    "OnAirNpeStateChanged",
                    vec![PyValue::Int(c), PyValue::Int(COMPLETED)],
                );
                return Some((ticket, Kw::new()));
            }
            let outputs = perform(reg, c, &atom, &args);
            if let Some(o) = &outputs {
                eprintln!(
                    "[airnpe] char {c}: {atom} {} -> {}",
                    describe(&args),
                    describe(o)
                );
            }
            return Some((ticket, outputs.unwrap_or_default()));
        }
    }
    None
}

/// The server atoms [`apply`] performs; any other is logged once per run.
const HANDLED: &[&str] = &[
    "Complete AIR NPE",
    "Create Item in Hangar",
    "Give Skill",
    "Give Skill Points",
    "Give ISK",
    "Spawn Dungeon Near Undock",
    "Warp Character To Position",
    "Remove Item from Cargo",
];

fn describe(args: &Kw) -> String {
    args.iter()
        .filter(|(k, _)| *k != "character_id")
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn int(args: &Kw, key: &str) -> Option<i64> {
    args.get(key).and_then(Value::as_int)
}

/// The server atoms that reward the pilot, on the server's own systems.
/// `None` for one this module does not perform (the caller logs it once);
/// `Some` with the atom's outputs, if it has any, for one it does.
/// What each takes is the atom's own parameter list **[V-data]** (`atoms`);
/// what it does is its name, and the client's lessons that read the result.
fn perform(reg: &mut Registry, c: i64, atom: &str, args: &Kw) -> Option<Kw> {
    match atom {
        // `type_id`, `should_only_create_if_missing`: one item in the
        // character's station hangar.
        "Create Item in Hangar" => {
            let Some(type_id) = int(args, "type_id") else {
                return Some(Kw::new());
            };
            let only_if_missing = args
                .get("should_only_create_if_missing")
                .is_some_and(Value::truthy);
            if only_if_missing
                && reg
                    .store()
                    .items()
                    .iter()
                    .any(|i| i.owner_id == c && i.type_id == type_id)
            {
                return Some(Kw::new());
            }
            let Some(station) = reg.character(c).and_then(|ch| ch.station_id) else {
                return Some(Kw::new());
            };
            let to = crate::item::Slot::new(c, station, crate::model::FLAG_HANGAR);
            if let Err(e) = reg.apply_items(&[crate::item::Edit::Create {
                type_id,
                quantity: 1,
                to,
            }]) {
                eprintln!("[airnpe] char {c}: Create Item in Hangar {type_id} refused: {e:?}");
            }
            Some(Kw::new())
        }
        // `type_id`, `level`: the skill, injected (level 0 is "known").
        // **[A]** a level above 0 is not trained to here; falsified by a
        // graph that gives one.
        "Give Skill" => {
            let Some(type_id) = int(args, "type_id") else {
                return Some(Kw::new());
            };
            let content = reg.content.clone();
            let known = reg.skills.get(&c).is_some_and(|s| s.get(type_id).is_some());
            if !known {
                if let Some(skills) = reg.skills.get_mut(&c) {
                    let _ = skills.purchase_in(&content, &[type_id]);
                }
                reg.mark_dirty();
                if let Some(push) = crate::skill_queue::skills_changed_push(reg, c, &[type_id]) {
                    reg.schedule(Duration::ZERO, push);
                }
                crate::brain::announce(reg, c);
            }
            Some(Kw::new())
        }
        // `skill_points`: unallocated points, the pool `free_sp` keeps.
        "Give Skill Points" => {
            if let Some(points) = int(args, "skill_points") {
                crate::free_sp::grant(reg, c, points);
            }
            Some(Kw::new())
        }
        // `isk_amount` from `corporation_id`. **[A]** journalled as an agent
        // mission reward; falsified by a live journal row of another type.
        "Give ISK" => {
            let amount = args
                .get("isk_amount")
                .and_then(Value::as_float)
                .unwrap_or(0.0);
            let payer = int(args, "corporation_id").unwrap_or(0);
            if amount > 0.0 {
                crate::service::corp_wallet::char_post(
                    reg,
                    crate::service::corp_wallet::Via::Clock,
                    c,
                    amount,
                    crate::mission_rewards::REF_AGENT_MISSION_REWARD,
                    payer,
                    c,
                    0,
                    "",
                );
            }
            Some(Kw::new())
        }
        _ => site_action(reg, c, atom, args),
    }
}

/// Start the pending tutorials that are due, fire due delays, release held
/// messages that waited too long.
pub fn tick(reg: &mut Registry, _now: Instant, bt: crate::damage::BlueTime) {
    site_tick(reg, bt);
    let due: Vec<i64> = {
        let runs = reg.ext::<Runs>();
        if runs.runs.is_empty() && runs.pending.is_empty() {
            return;
        }
        runs.pending
            .iter()
            .filter(|(_, at)| **at <= crate::game_clock::clock::wall())
            .map(|(c, _)| *c)
            .collect()
    };
    for c in due {
        reg.ext_mut::<Runs>().pending.remove(&c);
        start_run(reg, c);
    }
    let now = clock(reg);
    let chars: Vec<i64> = reg.ext::<Runs>().runs.keys().copied().collect();
    for c in chars {
        let Some(run) = reg.ext_mut::<Runs>().runs.get_mut(&c) else {
            continue;
        };
        let fired = run.rt.tick(now);
        let overdue = !run.ready
            && !run.held.is_empty()
            && crate::game_clock::clock::wall_since(run.started) >= HOLD_LIMIT;
        if fired || overdue || !run.rt.effects.is_empty() {
            deliver(reg, c);
        }
    }
}

/// On logoff the client's graphs go with the session; so does the run.
pub fn on_logoff(reg: &mut Registry, c: i64) {
    let runs = reg.ext_mut::<Runs>();
    runs.pending.remove(&c);
    if let Some(mut run) = runs.runs.remove(&c) {
        run.rt.stop();
    }
}

// -- /airnpe ----------------------------------------------------------------------

/// `/airnpe start [checkpoint] | stop | status`.
pub fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    let Some(c) = actor else {
        return Ok(say("no character"));
    };
    match args.first().copied().unwrap_or("status") {
        "start" => {
            if library(reg).is_none() {
                return Ok(say("no node graphs in the content bundle"));
            }
            let checkpoint = args.get(1).and_then(|s| s.parse().ok());
            put(reg, c, ACTIVE, checkpoint);
            start_run(reg, c);
            Ok(say(&format!(
                "AIR tutorial started at checkpoint {checkpoint:?}; relog if the client shows nothing"
            )))
        }
        "stop" => {
            on_logoff(reg, c);
            reg.deltas_mut().del(KIND, &key(c));
            reg.mark_dirty();
            Ok(say("AIR tutorial stopped and cleared"))
        }
        _ => {
            let s = stored(reg, c);
            let running = reg
                .ext::<Runs>()
                .runs
                .get(&c)
                .map(|r| (r.rt.instance_id().to_string(), r.ready, r.held.len()));
            Ok(say(&format!("state {s:?}, run {running:?}")))
        }
    }
}

fn say(text: &str) -> PyValue {
    PyValue::Unicode(text.to_string())
}

// -- the wire ------------------------------------------------------------------------

fn str_py(s: &str) -> PyValue {
    PyValue::Bytes(s.as_bytes().to_vec())
}

pub fn to_py(v: &Value) -> PyValue {
    match v {
        Value::None => PyValue::None,
        Value::Bool(b) => PyValue::Bool(*b),
        Value::Int(i) => PyValue::Int(*i),
        Value::Float(f) => PyValue::Real(*f),
        Value::Str(s) => str_py(s),
        Value::List(items) => PyValue::List(items.iter().map(to_py).collect()),
    }
}

pub fn from_py(v: &PyValue) -> Value {
    match v {
        PyValue::None => Value::None,
        PyValue::Bool(b) => Value::Bool(*b),
        PyValue::Int(i) | PyValue::Long(i) => Value::Int(*i),
        PyValue::Real(f) => Value::Float(*f),
        PyValue::Bytes(b) => Value::Str(String::from_utf8_lossy(b).into_owned()),
        PyValue::Unicode(s) => Value::Str(s.clone()),
        PyValue::List(items) | PyValue::Tuple(items) => {
            Value::List(items.iter().map(from_py).collect())
        }
        _ => Value::None,
    }
}

#[derive(Debug)]
struct Notice {
    character_id: i64,
    event: &'static str,
    args: Vec<PyValue>,
}

impl crate::notify::Push for Notice {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (self.event, self.args.clone())
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character_id])
    }
}

fn notify(reg: &mut Registry, c: i64, event: &'static str, args: Vec<PyValue>) {
    reg.schedule(
        Duration::ZERO,
        Notice {
            character_id: c,
            event,
            args,
        },
    );
}

// -- the module ------------------------------------------------------------------------

/// Everything this feature adds, as one module (`crate::module`): the bundle
/// table, the run slot, the state kind, the tick stage, the `node_graph`
/// service, `/airnpe` and the logoff hook.
pub const MODULE: crate::module::Module = crate::module::Module {
    name: "air_npe",
    version: 1,
    tables: &[TABLE],
    slots: &[SLOT],
    kinds: &[(KIND, "air_npe")],
    stages: &[crate::module::Placed {
        stage: crate::tick::Stage {
            name: "air_npe",
            run: tick,
            every: 1,
            phase: 0,
        },
        after: "site_spawn",
    }],
    services: &[(
        crate::service::node_graph::SERVICES,
        crate::service::node_graph::dispatch,
    )],
    commands: &[crate::slash::Command {
        name: "airnpe",
        usage: "/airnpe start [checkpoint] | stop | status",
        run: slash,
    }],
    on_logoff: &[crate::session_hooks::Row {
        name: "air_npe",
        run: on_logoff,
    }],
    ..crate::module::Module::NONE
};

// -- scene 1.4's site ----------------------------------------------------------------
//
// Task 1.4.01 sends the pilot to the system's sun, and the client's task graph
// waits for `Dungeon Entered` with one dungeon id and reads three of that
// dungeon's objects (`Get Dungeon Object`) **[V-data]** (the `[AIR] 1.4.01`
// client graph). No server graph spawns it: the `[AIR] 1.4` server graph only
// moves the checkpoint. On a live server the pilot who warped to the sun on
// this task landed in that dungeon's room **[V-TQ]**
// (`OnEnteringDungeonRoom(dungeon, room, ...)` right after the warp). So the
// rule is ours: a character at [`SITE_CHECKPOINT`] gets the dungeon laid out at
// the sun's warp-in point (`site::instantiate`, from `dungeon_data`), once
// they are there; `dungeon_tracking` then tells the client it is inside.

/// The task-1.4.01 dungeon **[V-data]** (the client task graph's `In Dungeon` id).
pub const SITE_DUNGEON: i64 = 10461;
/// The checkpoint the `[AIR] 1.4` server graph runs task 1.4.01 at **[V-data]**.
pub const SITE_CHECKPOINT: i64 = 45;
/// Site ids for these sites, two per character (the scene-1.4 site and a
/// spawned dungeon): above the sov stand-in hubs
/// (`8.9e9 + systemID`) and below escrow (9e9);
/// `site::tests::id_bands_do_not_overlap` checks it. **[A]** a character id
/// maps to one site by `rem_euclid`; falsified by two tutorial pilots whose
/// ids differ by a multiple of the band's width.
pub const NPE_SITE_BASE: i64 = 8_950_000_000;
pub const NPE_SITE_END: i64 = 9_000_000_000;

/// Whether `site.rs`'s sweep should leave this instance alone (it is ours to vacate).
pub fn is_npe_site(site_id: i64) -> bool {
    (NPE_SITE_BASE..NPE_SITE_END).contains(&site_id)
}

/// A character's scene-1.4 site: the even id of its pair.
fn site_id(c: i64) -> i64 {
    NPE_SITE_BASE + c.rem_euclid((NPE_SITE_END - NPE_SITE_BASE) / 2) * 2
}

/// A character's spawned dungeon (scene 2.1's mining site): the odd id.
fn spawn_site_id(c: i64) -> i64 {
    site_id(c) + 1
}

/// The character whose tutorial site this is, if one is running.
fn owner_of(w: &Registry, site: i64) -> Option<i64> {
    w.ext::<Runs>()
        .runs
        .keys()
        .copied()
        .find(|c| site_id(*c) == site || spawn_site_id(*c) == site)
}

// -- scene 2.1's mining site ---------------------------------------------------
//
// The `[AIR] 2.1 - Mining Dungeon Set-up` server graph spawns dungeon 9245
// near the home station's undock with `Spawn Dungeon Near Undock`, writes the
// spawn id, entry position and beacon to the blackboard, and reads the item id
// of one of its objects (a wreck) with `Get Item Id For Dungeon Object`; the
// scene graph then warps the pilot there with `Warp Character To Position`
// **[V-data]** (graphs 737 and 711). Each atom's parameters are its own
// (`atoms`); what it does is its name.

/// The scene-2.1 atoms. `None` for one not performed here (logged once):
/// the NPC behaviour writes (`Dungeon Behavior Blackboard Write`),
/// `Targetable`/`Untargetable`, `Cloak`/`Uncloak`, `Destroy Dungeon Object`
/// and `Despawn Dungeon When Empty` are not done yet.
fn site_action(reg: &mut Registry, c: i64, atom: &str, args: &Kw) -> Option<Kw> {
    match atom {
        "Spawn Dungeon Near Undock" => spawn_near_undock(reg, c, args),
        // `character_id`, `position`, `cheat_warp`: a warp the pilot did not
        // ask for, to a point.
        "Warp Character To Position" => {
            let to = vec3(args.get("position")?)?;
            crate::service::warp_character_to(reg, c, to);
            Some(Kw::new())
        }
        // `character_id`, `type_id`, `quantity_to_keep`: the cargo hold keeps
        // at most that many of the type.
        "Remove Item from Cargo" => {
            let type_id = int(args, "type_id")?;
            let keep = int(args, "quantity_to_keep").unwrap_or(0).max(0);
            remove_from_cargo(reg, c, type_id, keep);
            Some(Kw::new())
        }
        _ => None,
    }
}

fn vec3(v: &Value) -> Option<[f64; 3]> {
    match v {
        Value::List(items) if items.len() == 3 => {
            let f = |i: usize| items[i].as_float();
            Some([f(0)?, f(1)?, f(2)?])
        }
        _ => None,
    }
}

fn float(args: &Kw, key: &str) -> Option<f64> {
    args.get(key).and_then(Value::as_float)
}

/// `dungeon_id`, `solar_system_id`, `station_id`, `min_distance_au`,
/// `max_distance_au`, `max_angle_degrees` -> `spawn_id`, `entry_position`,
/// `beacon_id` **[V-data]** (the outputs graph 737 reads). The dungeon goes
/// straight out of the station's undock, at the middle of the distance
/// range. **[A]** a fixed point in the cone rather than a random one;
/// falsified by a lesson that depends on where in the cone it lands.
/// Spawned once per character; a second call answers the same site.
fn spawn_near_undock(reg: &mut Registry, c: i64, args: &Kw) -> Option<Kw> {
    let dungeon_id = int(args, "dungeon_id")?;
    let system = int(args, "solar_system_id")?;
    let station = int(args, "station_id")?;
    let id = spawn_site_id(c);
    if reg.ext::<crate::site::Sites>().get(id).is_none() {
        let st = *reg.celestials().get(station)?;
        let ship_type = reg.character(c).map_or(0, |ch| ch.ship_type_id);
        let content = reg.content();
        let out = crate::docking::undock_point(
            content.get::<crate::docking::Table>(),
            content,
            st.type_id,
            st.position,
            ship_type,
        )
        .map(|u| u.direction)
        .filter(|d| d.iter().any(|x| *x != 0.0))
        .unwrap_or_else(|| away_from_sun(st.position));
        let lo = float(args, "min_distance_au").unwrap_or(6.0);
        let hi = float(args, "max_distance_au").unwrap_or(lo);
        let d = (lo + hi) / 2.0 * crate::scan::AU;
        let anchor = [
            st.position[0] + out[0] * d,
            st.position[1] + out[1] * d,
            st.position[2] + out[2] * d,
        ];
        let archetype = content
            .try_get::<crate::dungeon_data::Table>()
            .and_then(|t| t.dungeons.get(&dungeon_id))
            .map_or(0, |d| d.archetype);
        let site = crate::population::Site {
            system,
            row: 0,
            k: 0,
            slot: 0,
            generation: 0,
            site_id: id,
            dungeon_id,
            archetype,
            entry: crate::dungeon_data::Entry::Anomaly,
            anchor,
            target_id: "AIR-002".into(),
            state: crate::population::State::Live,
            at: 0,
        };
        if !crate::site::instantiate(reg, &site) {
            return None;
        }
        eprintln!(
            "[airnpe] char {c}: dungeon {dungeon_id} spawned near the undock of {station} as site {id}"
        );
    }
    let inst = reg.ext::<crate::site::Sites>().get(id)?.clone();
    let entry = reg
        .content()
        .try_get::<crate::dungeon_data::Table>()
        .and_then(|t| t.dungeons.get(&inst.dungeon_id))
        .and_then(|d| d.entry_object);
    let beacon = inst
        .objects
        .iter()
        .find(|p| Some(p.dun_object_id) == entry)
        .or(inst.objects.first())
        .map_or(Value::None, |p| Value::Int(p.item_id));
    Some(Kw::from([
        ("spawn_id".to_string(), Value::Int(id)),
        (
            "entry_position".to_string(),
            Value::List(inst.anchor.iter().map(|x| Value::Float(*x)).collect()),
        ),
        ("beacon_id".to_string(), beacon),
    ]))
}

/// The unit vector from the sun (the origin) through `p`.
fn away_from_sun(p: [f64; 3]) -> [f64; 3] {
    let n = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
    if n == 0.0 {
        [1.0, 0.0, 0.0]
    } else {
        [p[0] / n, p[1] / n, p[2] / n]
    }
}

/// `(spawn id, dungeon object id) -> item id` for `c`'s spawned dungeon,
/// what `Get Item Id For Dungeon Object` answers.
fn spawned_objects(reg: &Registry, c: i64) -> HashMap<(i64, i64), i64> {
    let id = spawn_site_id(c);
    reg.ext::<crate::site::Sites>()
        .get(id)
        .map(|i| {
            i.objects
                .iter()
                .map(|p| ((id, p.dun_object_id), p.item_id))
                .collect()
        })
        .unwrap_or_default()
}

/// Consume `type_id` in `c`'s active ship's cargo hold down to `keep` units.
fn remove_from_cargo(reg: &mut Registry, c: i64, type_id: i64, keep: i64) {
    let Some(ship) = reg.character(c).map(|ch| ch.ship_id) else {
        return;
    };
    let stacks: Vec<(i64, i64)> = reg
        .store()
        .contents_of(ship)
        .filter(|i| i.flag_id == crate::fitting::FLAG_CARGO && i.type_id == type_id)
        .map(|i| (i.item_id, i.quantity))
        .collect();
    let mut over = stacks.iter().map(|(_, q)| q).sum::<i64>() - keep;
    let mut edits = Vec::new();
    for (item_id, quantity) in stacks {
        if over <= 0 {
            break;
        }
        let take = quantity.min(over);
        over -= take;
        edits.push(if take == quantity {
            crate::item::Edit::Destroy { item_id }
        } else {
            crate::item::Edit::Consume {
                item_id,
                quantity: take,
            }
        });
    }
    if !edits.is_empty()
        && let Err(e) = reg.apply_items(&edits)
    {
        eprintln!("[airnpe] char {c}: Remove Item from Cargo {type_id} refused: {e:?}");
    }
}

fn npe_site(
    reg: &Registry,
    c: i64,
    system: i64,
    at: crate::damage::BlueTime,
) -> Option<crate::population::Site> {
    let sun = reg.celestials().star_of(system)?;
    let anchor = crate::warp_in::warp_in_point(sun).unwrap_or(sun.position);
    let archetype = reg
        .content()
        .try_get::<crate::dungeon_data::Table>()
        .and_then(|t| t.dungeons.get(&SITE_DUNGEON))
        .map_or(0, |d| d.archetype);
    Some(crate::population::Site {
        system,
        row: 0,
        k: 0,
        slot: 0,
        generation: 0,
        site_id: site_id(c),
        dungeon_id: SITE_DUNGEON,
        archetype,
        entry: crate::dungeon_data::Entry::Anomaly,
        anchor,
        target_id: "AIR-001".into(),
        state: crate::population::State::Live,
        at,
    })
}

fn site_tick(reg: &mut Registry, at: crate::damage::BlueTime) {
    let chars: Vec<i64> = reg.ext::<Runs>().runs.keys().copied().collect();
    for c in chars {
        let checkpoint = stored(reg, c).and_then(|(_, k)| k);
        let id = site_id(c);
        let live = reg.ext::<crate::site::Sites>().get(id).is_some();
        let want = checkpoint == Some(SITE_CHECKPOINT);
        if !want && !live {
            continue;
        }
        let Some(system) = reg.session_system(c) else {
            continue;
        };
        reg.advance_character(c);
        let pos = reg
            .roster()
            .get(c)
            .filter(|p| p.solar_system_id == system)
            .map(|p| p.motion.position);
        let Some(site) = npe_site(reg, c, system, at) else {
            continue;
        };
        let dist = pos.map(|p| crate::space_state::distance(p, site.anchor));
        let near = dist.is_some_and(|d| d <= crate::site::ARRIVAL_RANGE);
        if want && !live && near {
            if crate::site::instantiate(reg, &site) {
                eprintln!(
                    "[airnpe] char {c}: dungeon {SITE_DUNGEON} placed at the sun of {system} as site {id}"
                );
                stock(reg, c, id);
            }
        } else if live && !want && !near {
            crate::site::vacate(reg, id);
            eprintln!("[airnpe] char {c}: site {id} vacated");
        }
    }
}

/// The task's wrecks: the three dungeon objects the `[AIR] 1.4.01` client
/// graph reads with `Get Dungeon Object` **[V-data]**.
pub const TASK_WRECKS: [i64; 3] = [1_179_831, 1_179_888, 1_179_889];
/// What the task retrieves **[V-data]**: the client graph waits for
/// `entry_0.ItemEntry_<type>` in the open wreck, and the `[AIR] 1.5.01`
/// fitting graph fits the same type. **[A]** one per wreck (a live server's
/// wreck held one, `[V-TQ]`; falsified by a capture holding more).
pub const TASK_ITEM: i64 = 78_677;

/// One of a tutorial site's task wrecks, as a wreck its character may loot.
pub fn loot_source(w: &Registry, id: i64) -> Option<crate::combat::Wreck> {
    let inst = w.ext::<crate::site::Sites>().of_item(id)?;
    if !is_npe_site(inst.site_id) {
        return None;
    }
    let (p, system) = crate::site::object(w, id)?;
    if !TASK_WRECKS.contains(&p.dun_object_id) {
        return None;
    }
    let owner = owner_of(w, inst.site_id)?;
    Some(crate::combat::Wreck {
        wreck_id: id,
        type_id: p.type_id,
        solar_system_id: system,
        position: p.position,
        ship_id: id,
        owner_id: owner,
        corp_id: w.character(owner).map_or(owner, |c| c.corporation_id),
        fleet_id: crate::combat::fleet_of(w, owner),
        abandoned: false,
        expires_at: crate::damage::BlueTime::MAX,
    })
}

/// Put the task item in each task wreck of `c`'s site that holds nothing.
fn stock(reg: &mut Registry, c: i64, site: i64) {
    let wrecks: Vec<i64> = reg
        .ext::<crate::site::Sites>()
        .get(site)
        .map(|i| {
            i.objects
                .iter()
                .filter(|p| TASK_WRECKS.contains(&p.dun_object_id))
                .map(|p| p.item_id)
                .collect()
        })
        .unwrap_or_default();
    for wreck in wrecks {
        if reg.store().items().iter().any(|it| it.location_id == wreck) {
            continue;
        }
        let to = crate::item::Slot::new(c, wreck, crate::hacking_loot::FLAG_IN_CAN);
        match reg.apply_items_quiet(&[crate::item::Edit::Create {
            type_id: TASK_ITEM,
            quantity: 1,
            to,
        }]) {
            Ok(_) => eprintln!("[airnpe] char {c}: wreck {wreck} holds {TASK_ITEM}"),
            Err(e) => eprintln!("[airnpe] char {c}: stocking wreck {wreck} refused: {e:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(key: &str) -> Effect {
        Effect::ClientMessage {
            instance: "i".into(),
            key: key.into(),
            value: Value::str("v"),
        }
    }

    fn checkpoint(n: i64) -> Effect {
        Effect::Checkpoint {
            graph_id: 990_001,
            checkpoint: n,
        }
    }

    #[test]
    fn client_messages_wait_for_the_client_and_go_out_in_order() {
        let mut held = Vec::new();
        let out = release_or_hold(&mut held, vec![msg("a"), checkpoint(1), msg("b")], false);
        assert_eq!(out, vec![checkpoint(1)], "a checkpoint does not wait");
        assert_eq!(held, vec![msg("a"), msg("b")]);
        let out = release_or_hold(&mut held, vec![msg("c")], false);
        assert!(out.is_empty());
        let out = release_or_hold(&mut held, vec![msg("d"), checkpoint(2)], true);
        assert_eq!(
            out,
            vec![msg("a"), msg("b"), msg("c"), msg("d"), checkpoint(2)]
        );
        assert!(held.is_empty());
    }

    #[test]
    fn wire_values_round_trip() {
        let v = Value::List(vec![
            Value::None,
            Value::Bool(true),
            Value::Int(-3),
            Value::Float(1.5),
            Value::str("x"),
        ]);
        assert_eq!(from_py(&to_py(&v)), v);
        assert_eq!(from_py(&PyValue::Unicode("u".into())), Value::str("u"));
        assert_eq!(
            from_py(&PyValue::Tuple(vec![PyValue::Long(7)])),
            Value::List(vec![Value::Int(7)])
        );
    }

    #[test]
    fn every_character_gets_a_site_in_the_band() {
        for c in [
            90_000_001,
            2_112_000_000,
            -1,
            0,
            NPE_SITE_END - NPE_SITE_BASE,
        ] {
            assert!(is_npe_site(site_id(c)), "{c}");
            assert!(is_npe_site(spawn_site_id(c)), "{c}");
        }
        assert_ne!(site_id(90_000_001), site_id(90_000_002));
        assert_ne!(spawn_site_id(90_000_001), site_id(90_000_002));
        assert!(!is_npe_site(crate::population::SITE_BASE));
    }

    /// The module's names clash with no core row (the startup refusals).
    #[test]
    fn the_module_assembles_after_the_core() {
        crate::module::build(&[&MODULE]).expect("air_npe assembles");
    }
}
