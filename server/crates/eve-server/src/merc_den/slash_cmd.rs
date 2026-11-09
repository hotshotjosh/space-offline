//! `/mercden`, the GM line (spec L10.8). For tests and human checks; none of
//! it is a client route.
//!
//! - `info <id>`: the row, the levels and stages now, the infomorphs held;
//! - `age <id> <hours>`: move both `simulated_at`s back by whole hours, so
//!   server and client both see the elapsed time without waiting;
//! - `fill <id>`: the den holds its cargo cap of infomorphs now;
//! - `pause <id>` / `resume <id>`: the evolution, with its notice;
//! - `go <id>`: put the GM's ship at the den's surface;
//! - `generate <den>` / `complete <activity uuid>` / `list <den>`: the den's
//!   activities ([`super::activity`]);
//! - `remove <id>`: destroy the den's item (the clock removes the row and
//!   tells the owner, or [`tick_at`] right away here).

use super::*;
use crate::pyutil::bstr;

const USAGE: &str = "usage: /mercden info <id> | age <id> <hours> | fill <id> | pause <id> | resume <id> | go <id> | remove <id> | generate <den> | complete <activity uuid> | list <den>";

/// The `slash::COMMANDS` row `mercden` (coordinator).
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
    // `generate`, `complete` and `list`: the activities (MERC-2).
    if let Some(text) = super::activity::slash(reg, args, now) {
        return text;
    }
    match args {
        ["info", d] => id_of(d).map_or_else(|| USAGE.to_owned(), |id| info(reg, id, now)),
        ["age", d, hours] => match (id_of(d), hours.parse::<u64>()) {
            (Some(id), Ok(h)) if h > 0 => age(reg, id, h, now),
            _ => USAGE.to_owned(),
        },
        ["fill", d] => id_of(d).map_or_else(|| USAGE.to_owned(), |id| fill(reg, id, now)),
        ["pause", d] => id_of(d).map_or_else(|| USAGE.to_owned(), |id| pause(reg, id, true, now)),
        ["resume", d] => id_of(d).map_or_else(|| USAGE.to_owned(), |id| pause(reg, id, false, now)),
        ["go", d] => id_of(d).map_or_else(|| USAGE.to_owned(), |id| go(reg, actor, id)),
        ["remove", d] => id_of(d).map_or_else(|| USAGE.to_owned(), |id| destroy(reg, id, now)),
        _ => USAGE.to_owned(),
    }
}

fn info(reg: &Registry, id: i64, now: u64) -> String {
    let Some(den) = by_id(reg.deltas(), id) else {
        return format!("no mercenary den {id}");
    };
    let Some(def) = def_for(reg, &den) else {
        return format!("mercenary den {id} has no rules (type {})", den.type_id);
    };
    format!(
        "mercenary den {id}: owner {} skyhook {} planet {} system {} enabled={} paused={} \
         development level {} stage {} anarchy level {} stage {} infomorphs {}/{} last_gen {}",
        den.owner,
        den.skyhook,
        den.planet,
        den.system,
        den.enabled,
        den.paused,
        den.dev_level(&def, now),
        den.dev_stage(&def, now),
        den.anarchy_level(&def, now),
        den.anarchy_stage(&def, now),
        projected(&den, &def, now),
        def.cargo_cap,
        den.last_gen
    )
}

fn age(reg: &mut Registry, id: i64, hours: u64, now: u64) -> String {
    let Some(mut den) = settle(reg, id, now) else {
        return format!("no mercenary den {id}");
    };
    let back = hours.saturating_mul(3_600);
    den.dev_at = den.dev_at.saturating_sub(back);
    den.anarchy_at = den.anarchy_at.saturating_sub(back);
    put(reg.deltas_mut(), &den);
    announce_evolution(reg, &den, now);
    format!(
        "mercenary den {id} simulated_at moved back {hours} h{}",
        if den.paused {
            " (it is paused, so the levels do not rise)"
        } else {
            ""
        }
    )
}

fn fill(reg: &mut Registry, id: i64, now: u64) -> String {
    let Some(mut den) = by_id(reg.deltas(), id) else {
        return format!("no mercenary den {id}");
    };
    let Some(def) = def_for(reg, &den) else {
        return format!("mercenary den {id} has no rules");
    };
    den.infomorphs = def.cargo_cap;
    den.last_gen = now;
    put(reg.deltas_mut(), &den);
    format!("mercenary den {id} holds {} infomorphs", def.cargo_cap)
}

fn pause(reg: &mut Registry, id: i64, paused: bool, now: u64) -> String {
    match set_paused(reg, id, paused, now) {
        Some(_) => format!(
            "mercenary den {id} {}",
            if paused { "paused" } else { "resumed" }
        ),
        None => format!("no mercenary den {id}"),
    }
}

fn go(reg: &mut Registry, actor: Option<i64>, id: i64) -> String {
    let (Some(actor), Some(den)) = (actor, by_id(reg.deltas(), id)) else {
        return "no such mercenary den or no character".to_owned();
    };
    reg.advance_character(actor);
    let (Some(here), Some(at)) = (reg.roster().get(actor).copied(), position(reg, &den)) else {
        return "you are not in space".to_owned();
    };
    if here.solar_system_id != den.system {
        return "that mercenary den is in another system".to_owned();
    }
    let r = radius(reg, id);
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
    to.extend(reg.roster().others_in_system(den.system, actor));
    if let Some(update) = crate::destiny::ActionUpdate::new(to, stamp, actions) {
        reg.push(update);
    }
    crate::grid::reconcile_in(reg, den.system);
    format!("moved you to mercenary den {id}")
}

fn destroy(reg: &mut Registry, id: i64, now: u64) -> String {
    let Some(den) = by_id(reg.deltas(), id) else {
        return format!("no mercenary den {id}");
    };
    if let Err(e) = reg.apply_items_quiet(&[Edit::Destroy { item_id: den.id }]) {
        return format!("not destroyed: {e}");
    }
    tick_at(reg, now);
    format!("mercenary den {id} destroyed")
}
