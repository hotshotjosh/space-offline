//! `/skyhook`, the GM line (spec L8.10). The client already sends
//! `/skyhook state <id> <state>` from its GM menu (`menusvc.py:1226-1233`)
//! **[V]**; the rest are for tests and human checks.
//!
//! - `state <id> <n>`: set the `orbitalSkyhook.const` state the bracket shows;
//! - `spawn <planetID|nearest>`: an item and a row over a planet of the GM's
//!   system, owned by the GM's corp, unbuilt;
//! - `activate <id>`: finish the build now (or switch an idle one on);
//! - `age <id> <cycles>`: credit whole cycles of harvest now and leave
//!   `last_cycle` at now, so server and client both see the stock without
//!   waiting (a running skyhook only);
//! - `go <id>`: put the GM's ship at the skyhook's surface;
//! - `info <id>`: the row and the stocks now;
//! - `window <id> now`: open a theft window on the skyhook right now (SKY-2);
//! - `link finish <id>`: complete the running silo link now (SKY-2).

use super::*;
use crate::pyutil::bstr;

const USAGE: &str = "usage: /skyhook state <id> <state> | spawn <planetID|nearest> | activate <id> | age <id> <cycles> | go <id> | info <id> | window <id> now | link finish <id>";

/// The `slash::COMMANDS` row `skyhook` (coordinator).
pub fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    Ok(bstr(&run(reg, actor, args, now())))
}

fn id_of(s: &str) -> Option<i64> {
    s.parse().ok()
}

/// The command at `now`, as the text it answers.
pub fn run(reg: &mut Registry, actor: Option<i64>, args: &[&str], now: u64) -> String {
    match args {
        ["state", sk, n] => match (id_of(sk), n.parse::<u32>()) {
            (Some(id), Ok(n)) if STATES.contains(&n) => set_state(reg, id, n, now),
            (Some(_), Ok(_)) => format!("{USAGE} -- {n} is not a skyhook state"),
            _ => USAGE.to_owned(),
        },
        ["spawn", which] => spawn(reg, actor, which, now),
        ["activate", sk] => id_of(sk).map_or_else(|| USAGE.to_owned(), |id| activate(reg, id, now)),
        ["age", sk, cycles] => match (id_of(sk), cycles.parse::<u64>()) {
            (Some(id), Ok(n)) if n > 0 => age(reg, id, n, now),
            _ => USAGE.to_owned(),
        },
        ["go", sk] => id_of(sk).map_or_else(|| USAGE.to_owned(), |id| go(reg, actor, id)),
        ["window", sk, "now"] => id_of(sk).map_or_else(
            || USAGE.to_owned(),
            |id| crate::skyhook_theft::dev_window(reg, id, now),
        ),
        ["link", "finish", sk] => id_of(sk).map_or_else(
            || USAGE.to_owned(),
            |id| crate::skyhook_theft::dev_finish(reg, id, now),
        ),
        ["info", sk] => id_of(sk).map_or_else(|| USAGE.to_owned(), |id| info(reg, id, now)),
        _ => USAGE.to_owned(),
    }
}

fn set_state(reg: &mut Registry, id: i64, state: u32, now: u64) -> String {
    let Some(mut sk) = by_id(reg.deltas(), id) else {
        return format!("no skyhook {id}");
    };
    let before = crate::sov_workforce::before(reg, sk.system, false);
    sk.state = state;
    put(reg.deltas_mut(), &sk);
    changed_since(reg, &sk, now, &before);
    format!("skyhook {id} is now in state {state}")
}

/// The type of the first skyhook in the type table.
fn skyhook_type(reg: &Registry) -> Option<i64> {
    reg.content()
        .try_get::<crate::typedata::Installed>()?
        .table()
        .types()
        .find(|t| t.group_id == GROUP_SKYHOOK)
        .map(|t| t.type_id)
}

fn spawn(reg: &mut Registry, actor: Option<i64>, which: &str, now: u64) -> String {
    let Some(actor) = actor else {
        return "no character".to_owned();
    };
    reg.advance_character(actor);
    let (Some(here), Some(corp)) = (
        reg.roster().get(actor).copied(),
        reg.character(actor).map(|c| c.corporation_id),
    ) else {
        return "you are not in space".to_owned();
    };
    let system = here.solar_system_id;
    let free = |reg: &Registry, p: i64| {
        at_planet(reg.deltas(), p).is_none() && sov_resources::planet_yield(reg, p).is_some()
    };
    let planet = if *which == *"nearest" {
        planets(reg, system)
            .into_iter()
            .filter(|(p, _)| free(reg, *p))
            .map(|(p, at)| (p, crate::space_state::distance(here.motion.position, at)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(p, _)| p)
    } else {
        id_of(which).filter(|p| planets(reg, system).iter().any(|(q, _)| q == p) && free(reg, *p))
    };
    let Some(planet) = planet else {
        return "no free planet with a yield there".to_owned();
    };
    let Some(ty) = skyhook_type(reg) else {
        return "the type table has no skyhook".to_owned();
    };
    let changes = match reg.apply_items(&[Edit::Create {
        type_id: ty,
        quantity: 1,
        to: Slot::new(corp, system, FLAG_IN_SPACE),
    }]) {
        Ok(c) => c,
        Err(e) => return format!("not made: {e}"),
    };
    let id = created(&changes, 0);
    if id == 0 {
        return "not made".to_owned();
    }
    let before = crate::sov_workforce::before(reg, system, false);
    place(reg, id, planet, system, corp, now);
    let sk = by_id(reg.deltas(), id).expect("just placed");
    changed_since(reg, &sk, now, &before);
    format!("skyhook {id} over planet {planet}, building for {ACTIVATION_S} s")
}

fn activate(reg: &mut Registry, id: i64, now: u64) -> String {
    let Some(sk) = by_id(reg.deltas(), id) else {
        return format!("no skyhook {id}");
    };
    if !sk.built() {
        finish_build(reg, id, now);
        return format!("skyhook {id} built and active");
    }
    if sk.active {
        return format!("skyhook {id} is already active");
    }
    let before = crate::sov_workforce::before(reg, sk.system, false);
    let mut sk = sk;
    sk.active = true;
    sk.last_cycle = now;
    put(reg.deltas_mut(), &sk);
    changed_since(reg, &sk, now, &before);
    format!("skyhook {id} is active")
}

fn age(reg: &mut Registry, id: i64, cycles: u64, now: u64) -> String {
    let Some(sk) = settle(reg, id, now) else {
        return format!("no skyhook {id}");
    };
    let Some(r) = reagent_of(reg, sk.planet) else {
        return format!("skyhook {id} has no reagent to age");
    };
    let mut sk = sk;
    // Credit the cycles now and keep the settled `last_cycle` (the partial
    // cycle's phase): moving it back would be undone by the claim-time and join-time clamps in `live`, which
    // are seconds old in a scenario **[V]** (ci-1110 fci_skyhook, "holds
    // nothing to take"). A skyhook that is not running earns nothing.
    let runs = running(reg.deltas(), &sk);
    if runs {
        let back = Sim {
            last_cycle: sk
                .last_cycle
                .saturating_sub(cycles.saturating_mul(r.cycle_s)),
            ..sk.sim()
        };
        let s = harvest(&back, &r, now);
        sk.secured = s.secured;
        sk.unsecured = s.unsecured;
        sk.last_cycle = s.last_cycle;
    }
    put(reg.deltas_mut(), &sk);
    // The whole-system notice goes first and the stocks notice last: an
    // `AllInSolarSystemNotice` re-primes the client's cache wholesale, and
    // an `activate` just before queued one built with stock 0, so the flush
    // must end with a snapshot that carries the aged stock **[I]**
    // (ci-1111 fci_skyhook_raid, "unsecured 0"; falsified if the raid's
    // precondition still reads 0).
    crate::skyhook::announce_all(reg, sk.system, now);
    announce_stocks(reg, &sk, now);
    format!(
        "skyhook {id} credited {cycles} cycles of {} s{}",
        r.cycle_s,
        if runs {
            ""
        } else {
            " (it is not running, so nothing accrues)"
        }
    )
}

fn go(reg: &mut Registry, actor: Option<i64>, id: i64) -> String {
    let (Some(actor), Some(sk)) = (actor, by_id(reg.deltas(), id)) else {
        return "no such skyhook or no character".to_owned();
    };
    reg.advance_character(actor);
    let (Some(here), Some(at)) = (reg.roster().get(actor).copied(), position(reg, &sk)) else {
        return "you are not in space".to_owned();
    };
    if here.solar_system_id != sk.system {
        return "that skyhook is in another system".to_owned();
    }
    let r = radius(reg.content(), type_of(reg, id).unwrap_or(0));
    let point = [at[0] + r + 500.0, at[1], at[2]];
    let actions = vec![
        crate::destiny::Action::SetBallPosition {
            ball: here.ship_id,
            point,
        },
        crate::destiny::Action::SetBallVelocity {
            ball: here.ship_id,
            velocity: [0.0; 3],
        },
        crate::destiny::Action::Stop { ball: here.ship_id },
    ];
    if let Some(p) = reg.roster_mut().get_mut(actor) {
        for action in &actions {
            p.motion.after(*action);
        }
    }
    let stamp = reg.next_destiny_stamp();
    let mut to = vec![actor];
    to.extend(reg.roster().others_in_system(sk.system, actor));
    if let Some(update) = crate::destiny::ActionUpdate::new(to, stamp, actions) {
        reg.push(update);
    }
    crate::grid::reconcile_in(reg, sk.system);
    format!("moved you to skyhook {id}")
}

fn info(reg: &Registry, id: i64, now: u64) -> String {
    let Some(sk) = by_id(reg.deltas(), id) else {
        return format!("no skyhook {id}");
    };
    let v = view(reg, &sk, now);
    let stock = v.reagent.as_ref().map_or_else(
        || "no reagent".to_owned(),
        |(r, s)| {
            format!(
                "reagent {}: secured {} unsecured {} last_cycle {}",
                r.type_id, s.secured, s.unsecured, sk.last_cycle
            )
        },
    );
    format!(
        "skyhook {id}: planet {} system {} corp {} {} built={} state={} {stock}",
        sk.planet,
        sk.system,
        sk.corp,
        match (sk.active, running(reg.deltas(), &sk)) {
            (_, true) => "active running",
            (true, false) => "active not-running (system not held)",
            (false, _) => "inactive",
        },
        sk.built(),
        slim_state(&sk)
    )
}
