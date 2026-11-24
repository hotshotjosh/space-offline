//! Factional warfare battlefield scoreboards (#8, FW-7): the score a
//! battlefield (archetype 68) keeps between the two militias, and its win.
//!
//! **What the client does** ([V] `spacecomponents/client/components/
//! fwScoreboard.py`, build 3731714, identical bytecode in 3720952). The
//! component `fwScoreboard` is attached by type id from the client's own
//! `spacecomponentsbytype`; its two optional ints `attackerWinsThreshold` and
//! `occupierWinsThreshold` live there, not in our bundle until
//! `facwar/scoreboards.tsv` is extracted ([`crate::facwar::Table::scoreboard_of`]).
//! The client reads the score from the scoreboard ball's slim key
//! [`KEY_SCORE`] (`OnAddedToSpace` reads `slimItem.component_fwScoreboard_score
//! or 0`; `OnSlimItemUpdated` re-reads it). The one server push is
//! `OnFwScoreboardWon(solarSystemID, scoreboardItemID, winnerFactionID)`, a
//! fanfare only. The gauge is `pts = abs(occ) + att` and `progress =
//! (abs(occ) + score) / pts`, so the score is one signed int, positive toward
//! the attacker and negative toward the occupier, in `[-abs(occ), +att]`
//! ([`client_progress`]).
//!
//! **What this module decides ([I] from the gauge, [A] where marked).**
//!
//! - The win: `score >= att` is the attacker's, `score <= -abs(occ)` the
//!   occupier's ([`winner`]) **[I]**: the gauge is full at either end.
//! - [`credit`] is the only place the score moves: the attacker's side adds,
//!   the occupier's subtracts, anyone else is ignored; the score is clamped
//!   to the gauge's range and frozen after a win.
//! - Capture points score: a looping point's award and a non-looping point's
//!   capture at a battlefield go through [`credit`] instead of the system's
//!   victory points (`fw_capture::tick`) **[A]**.
//! - Kills score: [`on_kill`] gives [`KILL_POINTS`] to the killer's militia for
//!   killing a pilot of the other side within [`crate::site::ARRIVAL_RANGE`] of
//!   the board; NPC kills and capsules do not score **[A]**.
//! - A win ([`win`]): [`WON_EVENT`] goes to the characters in the system
//!   (audience **[A]**), the system's victory points move by
//!   `facwar::vp_for(68)` toward the winner **[A]**, and the population slot
//!   completes (once). No LP **[A]**. `facwar::tick`'s completion scan skips
//!   archetype 68, so the win is the only credit.
//! - If the system changed hands (the two sides swapped), the score is negated
//!   **[A]**: it is the same contest seen from the other side.
//!
//! **The values.** [`ASSUMED`] thresholds (20 each) stand when the bundle has
//! no row for the type or the row leaves one out **[A]**, falsified by the
//! client's `spacecomponentsbytype` row for a scoreboard type (the gauge in
//! the info panel then disagrees with the win). [`KILL_POINTS`] **[A]**: one
//! point a kill; falsified by a battlefield that is won in an order of
//! magnitude more or fewer kills than the players expect.
//!
//! **Persisted** as the delta kind [`KIND`] (`"fwsb"`), key `[system,
//! site_id]`, value `score sides_occupier won_faction won_at`. The item id
//! is not saved; [`tick`] re-binds it each time the site's instance is laid
//! out again.

use crate::content::Boxed;
use crate::damage::BlueTime;
use crate::service::Registry;
use crate::world_ext::SlotSpec;
use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// The component name in `npc/components.tsv` and the client.
pub const COMPONENT: &str = "fwScoreboard";

/// The slim key the client reads the score from ([V]).
pub const KEY_SCORE: &str = "component_fwScoreboard_score";

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "fwsb";

/// The notify the client plays a fanfare on ([V], arity 3).
pub const WON_EVENT: &str = "OnFwScoreboardWon";

/// `dunArchetypeFactionaWarfareBattlefieldSites` [V].
pub const BATTLEFIELD: i64 = 68;

/// **[A]** Points one kill is worth. Falsified by a battlefield won in an
/// order of magnitude more or fewer kills than its players expect.
pub const KILL_POINTS: i64 = 1;

/// The win thresholds, as positive magnitudes: the attacker wins at
/// `+attacker`, the occupier at `-occupier`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    pub attacker: i64,
    pub occupier: i64,
}

/// **[A]** What stands when the bundle has no usable threshold: 20 each.
/// Falsified by the client's own `fwScoreboard` attributes of the type.
pub const ASSUMED: Thresholds = Thresholds {
    attacker: 20,
    occupier: 20,
};

/// The thresholds from the raw `(attackerWinsThreshold, occupierWinsThreshold)`
/// of the bundle. The sign is dropped (the client's gauge takes `abs` of the
/// occupier's, and the attacker's is a distance); a missing or zero one is
/// [`ASSUMED`]'s.
pub fn thresholds(raw: Option<(Option<i64>, Option<i64>)>) -> Thresholds {
    let (a, o) = raw.unwrap_or((None, None));
    let norm = |v: Option<i64>, fallback: i64| {
        v.map(i64::saturating_abs)
            .filter(|v| *v > 0)
            .unwrap_or(fallback)
    };
    Thresholds {
        attacker: norm(a, ASSUMED.attacker),
        occupier: norm(o, ASSUMED.occupier),
    }
}

/// Which side won.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Attacker,
    Occupier,
}

/// **[I]** The win rule: the gauge's two ends.
pub fn winner(score: i64, th: Thresholds) -> Option<Side> {
    if score >= th.attacker {
        Some(Side::Attacker)
    } else if score <= -th.occupier {
        Some(Side::Occupier)
    } else {
        None
    }
}

/// The score kept inside the gauge: `[-occupier, +attacker]`.
pub fn clamp(score: i64, th: Thresholds) -> i64 {
    score.clamp(-th.occupier, th.attacker)
}

/// The client's gauge, `fwScoreboard.py` [V]: `(abs(occ) + score) / (abs(occ) +
/// att)`. Tests only: it pins [`clamp`] and [`winner`] to the gauge.
#[cfg(test)]
pub fn client_progress(score: i64, th: Thresholds) -> f64 {
    (th.occupier + score) as f64 / (th.occupier + th.attacker) as f64
}

/// One live scoreboard.
#[derive(Debug, Clone, PartialEq)]
pub struct Board {
    pub site_id: i64,
    pub system: i64,
    /// The placed scoreboard object; re-bound by [`tick`].
    pub item_id: Option<i64>,
    pub type_id: i64,
    pub position: [f64; 3],
    pub attacker: i64,
    pub occupier: i64,
    /// Signed: positive toward the attacker.
    pub score: i64,
    /// The militia that won, once.
    pub won: Option<i64>,
    pub won_at: BlueTime,
}

/// The world state.
#[derive(Debug, Clone, Default)]
pub struct State {
    boards: BTreeMap<i64, Board>,
    next_check: BlueTime,
}

impl State {
    /// The board of a site, if one is live.
    pub fn board(&self, site_id: i64) -> Option<&Board> {
        self.boards.get(&site_id)
    }

    /// Every live board, by site.
    pub fn boards(&self) -> impl Iterator<Item = &Board> {
        self.boards.values()
    }
}

/// The registration row in `world_ext::SLOTS` (coordinator-only).
pub const SLOT: SlotSpec = SlotSpec {
    name: "fw_scoreboard",
    make: |_| Box::new(State::default()) as Boxed,
};

/// `OnFwScoreboardWon(solarSystemID, scoreboardItemID, winnerFactionID)`
/// ([V] arity 3), to the characters in the system.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreboardWon {
    pub to: Vec<i64>,
    pub system: i64,
    pub item: i64,
    pub winner: i64,
}

impl crate::notify::Push for ScoreboardWon {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            WON_EVENT,
            vec![
                PyValue::Int(self.system),
                PyValue::Int(self.item),
                PyValue::Int(self.winner),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// The thresholds of a scoreboard type, from the bundle or [`ASSUMED`].
pub fn thresholds_of(reg: &Registry, type_id: i64) -> Thresholds {
    thresholds(
        reg.content()
            .try_get::<crate::facwar::Table>()
            .and_then(|t| t.scoreboard_of(type_id)),
    )
}

/// Whether live site `site_id` is a battlefield: its dungeon's archetype is
/// [`BATTLEFIELD`]. (Not whether a board exists yet: [`tick`] makes boards.)
pub fn is_battlefield(reg: &Registry, site_id: i64) -> bool {
    reg.ext::<crate::site::Sites>()
        .get(site_id)
        .and_then(|i| {
            reg.content()
                .try_get::<crate::dungeon_data::Table>()?
                .dungeons
                .get(&i.dungeon_id)
        })
        .is_some_and(|d| d.archetype == BATTLEFIELD)
}

fn key(system: i64, site_id: i64) -> Vec<String> {
    vec![system.to_string(), site_id.to_string()]
}

fn store(reg: &mut Registry, b: &Board) {
    let value = vec![
        b.score.to_string(),
        b.occupier.to_string(),
        b.won.map_or_else(String::new, |f| f.to_string()),
        if b.won.is_some() {
            b.won_at.to_string()
        } else {
            String::new()
        },
    ];
    reg.deltas_mut().put(KIND, key(b.system, b.site_id), value);
}

/// A saved row: `(score, sides_occupier, won, won_at)`.
type Saved = (i64, i64, Option<i64>, BlueTime);

fn saved(reg: &Registry, system: i64, site_id: i64) -> Option<Saved> {
    let v = reg.deltas().of(KIND).get(&key(system, site_id))?;
    let int = |i: usize| v.get(i).and_then(|s| s.parse::<i64>().ok());
    Some((int(0)?, int(1)?, int(2), int(3).unwrap_or(BlueTime::MIN)))
}

/// `OnSlimItemChange` for a board's object: the client re-reads the score.
fn push_slim(reg: &mut Registry, b: &Board) {
    if let Some(item) = b.item_id {
        crate::fw_capture::push_slim(reg, item);
    }
}

/// A live battlefield: site, system, scoreboard object `(item, type,
/// position)`.
type Live = (i64, i64, (i64, i64, [f64; 3]));

/// **The stage** (called from `facwar::tick` after `fw_capture::tick`): every
/// [`crate::fw_capture::CHECK_EVERY`], boards are made for the live
/// battlefields of the warzone systems (their saved rows read lazily), item
/// ids are re-bound, a board whose site has gone is dropped, and a board
/// whose system changed hands has its score negated **[A]**.
pub fn tick(reg: &mut Registry, ft: BlueTime) {
    if ft < reg.ext::<State>().next_check {
        return;
    }
    reg.ext_mut::<State>().next_check = ft + crate::fw_capture::CHECK_EVERY;
    let live: Vec<Live> = {
        let npc = reg.content().try_get::<crate::npc_data::Table>();
        let dungeons = reg.content().try_get::<crate::dungeon_data::Table>();
        reg.ext::<crate::site::Sites>()
            .iter()
            .filter(|i| {
                reg.ext::<crate::facwar::Facwar>()
                    .system(i.system)
                    .is_some()
            })
            .filter(|i| {
                dungeons
                    .and_then(|t| t.dungeons.get(&i.dungeon_id))
                    .is_some_and(|d| d.archetype == BATTLEFIELD)
            })
            .filter_map(|i| {
                let o = i
                    .objects
                    .iter()
                    .find(|o| npc.is_some_and(|n| n.has_component(o.type_id, COMPONENT)))?;
                Some((i.site_id, i.system, (o.item_id, o.type_id, o.position)))
            })
            .collect()
    };
    let ids: std::collections::BTreeSet<i64> = live.iter().map(|l| l.0).collect();
    reg.ext_mut::<State>()
        .boards
        .retain(|id, _| ids.contains(id));
    for (site_id, system, (item, type_id, position)) in live {
        let Some(o) = crate::facwar::occupation(reg, system) else {
            continue;
        };
        let th = thresholds_of(reg, type_id);
        let mut b = match reg.ext::<State>().boards.get(&site_id) {
            Some(b) => b.clone(),
            None => {
                let (score, sides, won, won_at) =
                    saved(reg, system, site_id).unwrap_or((0, o.occupier, None, BlueTime::MIN));
                Board {
                    site_id,
                    system,
                    item_id: Some(item),
                    type_id,
                    position,
                    // The saved occupier, so a flip while the server was down
                    // is seen below.
                    attacker: if sides == o.occupier {
                        o.attacker
                    } else {
                        o.occupier
                    },
                    occupier: sides,
                    score,
                    won,
                    won_at,
                }
            }
        };
        let was = b.clone();
        b.item_id = Some(item);
        b.type_id = type_id;
        b.position = position;
        if b.won.is_none() && b.occupier != o.occupier {
            b.score = -b.score;
        }
        if b.won.is_none() {
            b.score = clamp(b.score, th);
        }
        b.attacker = o.attacker;
        b.occupier = o.occupier;
        let moved = (b.score, b.occupier) != (was.score, was.occupier);
        reg.ext_mut::<State>().boards.insert(site_id, b.clone());
        if moved {
            store(reg, &b);
            push_slim(reg, &b);
        }
    }
}

/// **The only place the score moves.** `points` for `faction` on site
/// `site_id`'s board: the attacker's side adds, the occupier's subtracts, any
/// other faction is ignored. The score is clamped to the gauge, saved, shown
/// to the grid, and a gauge filled to either end is a [`win`]. `false` when
/// the site has no board (the caller falls back to the system's victory
/// points); `true` when the board took it, whether or not it moved.
pub fn credit(reg: &mut Registry, site_id: i64, faction: i64, points: i64, ft: BlueTime) -> bool {
    let Some(b) = reg.ext::<State>().boards.get(&site_id).cloned() else {
        return false;
    };
    if b.won.is_some() {
        return true;
    }
    let th = thresholds_of(reg, b.type_id);
    let delta = if faction == b.attacker {
        points
    } else if faction == b.occupier {
        points.saturating_neg()
    } else {
        return true;
    };
    let score = clamp(b.score.saturating_add(delta), th);
    if score == b.score {
        return true;
    }
    let mut b = b;
    b.score = score;
    reg.ext_mut::<State>().boards.insert(site_id, b.clone());
    store(reg, &b);
    push_slim(reg, &b);
    if let Some(side) = winner(score, th) {
        win(reg, site_id, side, ft);
    }
    true
}

/// A board's win: set once, then the fanfare to the system **[A]**, the
/// system's victory points toward the winner **[A]**, and the slot is done.
fn win(reg: &mut Registry, site_id: i64, side: Side, ft: BlueTime) {
    let Some(b) = reg.ext_mut::<State>().boards.get_mut(&site_id) else {
        return;
    };
    if b.won.is_some() {
        return;
    }
    let winner = match side {
        Side::Attacker => b.attacker,
        Side::Occupier => b.occupier,
    };
    b.won = Some(winner);
    b.won_at = ft;
    let b = b.clone();
    store(reg, &b);
    let to = crate::facwar::in_system(reg, b.system);
    reg.push(ScoreboardWon {
        to,
        system: b.system,
        item: b.item_id.unwrap_or(0),
        winner,
    });
    let vp = crate::facwar::vp_for(BATTLEFIELD).unwrap_or(0);
    let delta = match side {
        Side::Attacker => vp,
        Side::Occupier => -vp,
    };
    crate::facwar::add_vp(reg, b.system, delta, ft);
    crate::population::complete(reg, b.system, b.site_id, ft);
}

/// **The `kill_hooks::PLAYER` row.** In a warzone system with a live board:
/// the killer and the victim are enlisted pilots of the system's two sides,
/// on different ones, the victim's hull is not a capsule, and the killer is
/// within [`crate::site::ARRIVAL_RANGE`] of the board: [`KILL_POINTS`] to the
/// killer's militia **[A]**. NPC kills do not score **[A]**.
pub fn on_kill(reg: &mut Registry, ev: &crate::kill_hooks::KillEvent) {
    if ev.npc.is_some() || ev.ship_type == crate::damage::CAPSULE_TYPE_ID {
        return;
    }
    let Some(killer) = ev.killer else {
        return;
    };
    let Some(o) = crate::facwar::occupation(reg, ev.system) else {
        return;
    };
    let (Some(kf), Some(vf)) = (
        crate::service::facwar_mgr::my_faction(reg, killer, ev.when),
        crate::service::facwar_mgr::my_faction(reg, ev.victim_character, ev.when),
    ) else {
        return;
    };
    let sides = [o.attacker, o.occupier];
    if kf == vf || !sides.contains(&kf) || !sides.contains(&vf) {
        return;
    }
    let Some(at) = reg.roster().get(killer).map(|p| p.motion.position) else {
        return;
    };
    let site = reg
        .ext::<State>()
        .boards()
        .find(|b| {
            b.system == ev.system
                && b.won.is_none()
                && crate::space_state::distance(at, b.position) <= crate::site::ARRIVAL_RANGE
        })
        .map(|b| b.site_id);
    if let Some(site) = site {
        credit(reg, site, kf, KILL_POINTS, ev.when);
    }
}

/// The slim extras of scoreboard objects (`slim_extra::SOURCES` row): the
/// score, 0 at the start.
pub fn slim_extras(reg: &Registry, ids: &[i64]) -> crate::slim_extra::SlimExtras {
    reg.ext::<State>()
        .boards()
        .filter_map(|b| {
            let id = b.item_id.filter(|i| ids.contains(i))?;
            Some((id, vec![(KEY_SCORE, PyValue::Int(b.score))]))
        })
        .collect()
}

/// **The rollover step**: drop the `fwsb` row of every site the population no
/// longer lists. Returns the rows dropped.
pub fn on_rollover(reg: &mut Registry, day: u32) -> usize {
    crate::population::settle(reg);
    let t = crate::game_clock::day_start(reg, day);
    let keys: Vec<Vec<String>> = reg.deltas().of(KIND).keys().cloned().collect();
    let mut dropped = 0;
    for k in keys {
        let [Some(system), Some(site)] =
            [0, 1].map(|i| k.get(i).and_then(|s| s.parse::<i64>().ok()))
        else {
            reg.deltas_mut().del(KIND, &k);
            dropped += 1;
            continue;
        };
        let listed = crate::population::sites(reg, system, t)
            .sites
            .iter()
            .any(|s| s.site_id == site);
        if !listed {
            reg.deltas_mut().del(KIND, &k);
            dropped += 1;
        }
    }
    dropped
}

/// The live battlefield slots of the warzone systems that have no board:
/// `battlefield slot site <id> system <id> (no board yet)`.
fn unlaid_slots(w: &Registry, now: BlueTime) -> Vec<String> {
    let boards: std::collections::BTreeSet<i64> =
        w.ext::<State>().boards().map(|b| b.site_id).collect();
    let systems: Vec<i64> = w
        .ext::<crate::facwar::Facwar>()
        .systems()
        .map(|(s, _)| s)
        .collect();
    let mut out = Vec::new();
    for system in systems {
        for s in crate::population::sites(w, system, now).sites {
            if s.archetype == BATTLEFIELD
                && s.state == crate::population::State::Live
                && !boards.contains(&s.site_id)
            {
                out.push(format!(
                    "battlefield slot site {} system {} (no board yet)",
                    s.site_id, s.system
                ));
            }
        }
    }
    out
}

/// `/fwscore [list | add <n> | reset]`: GM lever for the scoreboards.
/// `list` shows every live board; `add <n>` gives the caller's militia `n`
/// points on the board of the caller's grid; `reset` zeroes every board.
pub fn slash(
    w: &mut Registry,
    actor: Option<i64>,
    words: &[&str],
) -> Result<PyValue, crate::service::ServiceError> {
    let say = |s: &str| Ok(crate::pyutil::bstr(s));
    let now = crate::pyutil::filetime_now();
    match words {
        [] | ["list"] => {
            let lines: Vec<String> = w
                .ext::<State>()
                .boards()
                .map(|b| {
                    let th = thresholds_of(w, b.type_id);
                    format!(
                        "battlefield site {} system {} item {} score {} attacker {} wins {} occupier {} wins {} won {}",
                        b.site_id,
                        b.system,
                        b.item_id.unwrap_or(0),
                        b.score,
                        b.attacker,
                        th.attacker,
                        b.occupier,
                        th.occupier,
                        b.won.map_or_else(|| "none".to_owned(), |f| f.to_string()),
                    )
                })
                .collect();
            // Battlefield slots with no board yet (nobody has come near the
            // site, so it is not laid out): the client scenario plans from them.
            let mut lines = lines;
            lines.extend(unlaid_slots(w, now));
            if lines.is_empty() {
                say("no battlefields")
            } else {
                say(&lines.join("\n"))
            }
        }
        ["add", n] => {
            let Ok(n) = n.parse::<i64>() else {
                return say("usage: /fwscore add <points>");
            };
            let Some(actor) = actor else {
                return say("no caller");
            };
            let Some(faction) = crate::service::facwar_mgr::my_faction(w, actor, now) else {
                return say("you are not in a militia");
            };
            let Some(at) = w
                .roster()
                .get(actor)
                .map(|p| (p.solar_system_id, p.motion.position))
            else {
                return say("you are not in space");
            };
            let site = w
                .ext::<State>()
                .boards()
                .find(|b| {
                    b.system == at.0
                        && crate::space_state::distance(at.1, b.position)
                            <= crate::site::ARRIVAL_RANGE
                })
                .map(|b| b.site_id);
            let Some(site) = site else {
                return say("no battlefield on your grid");
            };
            credit(w, site, faction, n, now);
            let score = w.ext::<State>().board(site).map_or(0, |b| b.score);
            say(&format!("site {site} score is now {score}"))
        }
        ["reset"] => {
            let sites: Vec<i64> = w.ext::<State>().boards().map(|b| b.site_id).collect();
            for s in &sites {
                if let Some(b) = w.ext_mut::<State>().boards.get_mut(s) {
                    b.score = 0;
                    b.won = None;
                    b.won_at = BlueTime::MIN;
                }
                if let Some(b) = w.ext::<State>().board(*s).cloned() {
                    store(w, &b);
                    push_slim(w, &b);
                }
            }
            say(&format!("{} battlefield(s) reset", sites.len()))
        }
        _ => say("usage: /fwscore [list | add <points> | reset]"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    //! Invented ids throughout; the thresholds are the invented `30` and `-40`
    //! (the occupier's stored negative, as the client may store it). No [A]
    //! value is asserted as a fact: the assumed thresholds are tested only as
    //! "what stands", through [`ASSUMED`] itself.
    use super::*;
    use crate::dungeon_data::{Dungeon, Entry};
    use crate::facwar::tests as fx;
    use crate::population;
    use crate::space_state::{Motion, Presence};

    pub(crate) const SB_TYPE: i64 = 990_001;
    pub(crate) const CP_TYPE: i64 = 990_101;
    const BF_A: i64 = 999_368;
    const BF_B: i64 = 999_369;
    const RAW_ATT: i64 = 30;
    const RAW_OCC: i64 = -40;
    const TH: Thresholds = Thresholds {
        attacker: 30,
        occupier: 40,
    };

    /// The facwar fixture world plus battlefield dungeons of both militias, a
    /// population row for them, the component rows and the thresholds.
    pub(crate) fn world() -> Registry {
        fx::world_with(|npc, dun, rows, sb| {
            npc.components.insert((SB_TYPE, COMPONENT.to_owned()));
            npc.components
                .insert((CP_TYPE, crate::fw_capture::COMPONENT.to_owned()));
            for (id, f) in [(BF_A, fx::A), (BF_B, fx::B)] {
                dun.dungeons.insert(
                    id,
                    Dungeon {
                        id,
                        archetype: BATTLEFIELD,
                        faction: Some(f),
                        difficulty: Some(4),
                        entry_type: Some(Entry::Anomaly.type_id()),
                        ..Dungeon::default()
                    },
                );
            }
            rows.push(population::Row {
                key: "fx_bf_low".to_owned(),
                band: crate::celestial::Band::Low,
                archetypes: vec![BATTLEFIELD],
                entry: Entry::Anomaly,
                count: 8.0,
                difficulties: population::Diffs::In(vec![4]),
                respawn_s: 600,
                ttl_s: 172_800,
                pochven: false,
            });
            sb.insert(SB_TYPE, (Some(RAW_ATT), Some(RAW_OCC)));
        })
    }

    /// A live battlefield of S1 (held by A, attacked by B) with its board.
    pub(crate) struct Bf {
        pub reg: Registry,
        pub site: i64,
        pub system: i64,
        pub item: i64,
        pub anchor: [f64; 3],
        pub now: BlueTime,
    }

    impl Bf {
        pub fn score(&self) -> i64 {
            self.reg
                .ext::<State>()
                .board(self.site)
                .expect("a board")
                .score
        }

        pub fn put(&mut self, who: i64, at: [f64; 3]) {
            self.reg.roster_mut().arrive(Presence::arriving(
                who,
                who + 100,
                self.system,
                Motion::arrived(at, [0.0; 3]),
            ));
        }
    }

    /// The battlefield: its scoreboard placed at the anchor, ME (militia B,
    /// the attacker) on the grid, the board made. `with_point` adds one
    /// capture point object beside the board.
    pub(crate) fn battlefield(with_point: bool) -> Bf {
        let mut reg = world();
        let now = reg.world().birth;
        let site = fx::fw_sites(&reg, fx::S1)
            .into_iter()
            .find(|s| s.archetype == BATTLEFIELD)
            .expect("a battlefield in S1");
        assert!(crate::site::instantiate(&mut reg, &site));
        let sb = crate::site::add_object(&mut reg, site.site_id, SB_TYPE, site.anchor)
            .expect("the scoreboard is placed");
        if with_point {
            crate::site::add_object(&mut reg, site.site_id, CP_TYPE, site.anchor)
                .expect("the point is placed");
        }
        crate::service::facwar_mgr::enlist(&mut reg, fx::ME, fx::B, now).expect("ME enlists");
        let mut bf = Bf {
            reg,
            site: site.site_id,
            system: fx::S1,
            item: sb.item_id,
            anchor: site.anchor,
            now,
        };
        bf.put(fx::ME, site.anchor);
        tick(&mut bf.reg, now);
        assert!(bf.reg.ext::<State>().board(bf.site).is_some(), "a board");
        bf
    }

    /// `now` plus `n` stage periods.
    fn at(now: BlueTime, n: i64) -> BlueTime {
        now + n * crate::fw_capture::CHECK_EVERY
    }

    fn won_pushes(reg: &mut Registry) -> Vec<(Vec<PyValue>, Option<Vec<i64>>)> {
        fx::pushes(reg, WON_EVENT)
    }

    #[test]
    fn thresholds_are_magnitudes_and_a_missing_one_is_the_assumed() {
        let t = thresholds(Some((Some(RAW_ATT), Some(RAW_OCC))));
        assert_eq!(t, TH, "a negative stored occupier threshold is normalised");
        assert_eq!(thresholds(Some((Some(RAW_ATT), Some(-RAW_OCC)))), TH);
        assert_eq!(thresholds(None), ASSUMED);
        assert_eq!(thresholds(Some((None, None))), ASSUMED);
        let half = thresholds(Some((None, Some(RAW_OCC))));
        assert_eq!((half.attacker, half.occupier), (ASSUMED.attacker, 40));
        assert_eq!(
            thresholds(Some((Some(0), Some(RAW_OCC)))).attacker,
            ASSUMED.attacker
        );
    }

    #[test]
    fn the_win_rule_is_the_two_ends_of_the_gauge_and_the_clamp_keeps_inside() {
        assert_eq!(winner(0, TH), None);
        assert_eq!(winner(TH.attacker - 1, TH), None);
        assert_eq!(winner(TH.attacker, TH), Some(Side::Attacker));
        assert_eq!(winner(-TH.occupier + 1, TH), None);
        assert_eq!(winner(-TH.occupier, TH), Some(Side::Occupier));
        assert_eq!(clamp(1_000, TH), TH.attacker);
        assert_eq!(clamp(-1_000, TH), -TH.occupier);
        assert_eq!(clamp(7, TH), 7);
    }

    #[test]
    fn the_client_gauge_is_empty_at_the_occupiers_end_and_full_at_the_attackers() {
        assert_eq!(client_progress(-TH.occupier, TH), 0.0);
        assert_eq!(client_progress(TH.attacker, TH), 1.0);
        let mid = client_progress(0, TH);
        assert!((mid - 40.0 / 70.0).abs() < 1e-12, "{mid}");
        // Every score the clamp allows is a gauge position in [0, 1], and a
        // win is exactly where the gauge ends.
        for s in -TH.occupier..=TH.attacker {
            let p = client_progress(s, TH);
            assert!((0.0..=1.0).contains(&p));
            assert_eq!(winner(s, TH).is_some(), p == 0.0 || p == 1.0, "{s}");
        }
    }

    #[test]
    fn credit_moves_the_score_toward_the_side_and_ignores_a_bystander() {
        let mut bf = battlefield(false);
        let (a, o, other) = (fx::B, fx::A, fx::C);
        assert_eq!(bf.score(), 0);
        assert!(
            credit(&mut bf.reg, bf.site, a, 5, at(bf.now, 1)),
            "attacker adds"
        );
        assert_eq!(bf.score(), 5);
        assert!(
            credit(&mut bf.reg, bf.site, o, 3, at(bf.now, 1)),
            "occupier subtracts"
        );
        assert_eq!(bf.score(), 2);
        assert!(
            credit(&mut bf.reg, bf.site, other, 9, at(bf.now, 1)),
            "taken, ignored"
        );
        assert_eq!(bf.score(), 2);
        assert!(
            !credit(&mut bf.reg, 900_001, a, 1, at(bf.now, 1)),
            "no board there"
        );
        assert_eq!(
            saved(&bf.reg, bf.system, bf.site),
            Some((2, fx::A, None, BlueTime::MIN))
        );
    }

    #[test]
    fn the_score_is_clamped_and_frozen_after_a_win() {
        let mut bf = battlefield(false);
        credit(&mut bf.reg, bf.site, fx::B, TH.attacker - 1, at(bf.now, 1));
        assert_eq!(bf.score(), TH.attacker - 1);
        assert!(won_pushes(&mut bf.reg).is_empty());
        credit(&mut bf.reg, bf.site, fx::B, 1_000, at(bf.now, 2));
        assert_eq!(bf.score(), TH.attacker, "clamped at the gauge's end");
        let won = bf.reg.ext::<State>().board(bf.site).and_then(|b| b.won);
        assert_eq!(won, Some(fx::B));
        credit(&mut bf.reg, bf.site, fx::A, 1_000, at(bf.now, 3));
        assert_eq!(bf.score(), TH.attacker, "frozen");
        // The occupier's end is its own win.
        let mut bf = battlefield(false);
        credit(&mut bf.reg, bf.site, fx::A, 1_000, at(bf.now, 1));
        assert_eq!(bf.score(), -TH.occupier);
        let won = bf.reg.ext::<State>().board(bf.site).and_then(|b| b.won);
        assert_eq!(won, Some(fx::A));
    }

    #[test]
    fn a_win_pushes_once_to_the_system_with_system_item_and_winner() {
        let mut bf = battlefield(false);
        let _ = won_pushes(&mut bf.reg);
        credit(&mut bf.reg, bf.site, fx::B, 1_000, at(bf.now, 1));
        let told = won_pushes(&mut bf.reg);
        assert_eq!(told.len(), 1, "{told:?}");
        assert_eq!(
            told[0].0,
            vec![
                PyValue::Int(bf.system),
                PyValue::Int(bf.item),
                PyValue::Int(fx::B)
            ]
        );
        assert_eq!(told[0].1.as_deref(), Some(&[fx::ME][..]), "the system");
        credit(&mut bf.reg, bf.site, fx::B, 1_000, at(bf.now, 2));
        assert!(won_pushes(&mut bf.reg).is_empty(), "once");
    }

    #[test]
    fn a_win_moves_the_systems_vp_and_completes_the_slot_once() {
        let mut bf = battlefield(false);
        let vp = crate::facwar::vp_for(BATTLEFIELD).expect("battlefields are worth points");
        let slot = |bf: &Bf| {
            population::sites(&bf.reg, bf.system, bf.now)
                .sites
                .into_iter()
                .find(|s| s.site_id == bf.site)
                .map(|s| s.state)
        };
        assert_eq!(slot(&bf), Some(population::State::Live));
        credit(&mut bf.reg, bf.site, fx::B, 1_000, at(bf.now, 1));
        assert_eq!(
            crate::facwar::occupation(&bf.reg, bf.system).map(|o| o.score),
            Some(vp),
            "the attacker's win raises the score"
        );
        assert_eq!(slot(&bf), Some(population::State::Done));
        credit(&mut bf.reg, bf.site, fx::B, 1_000, at(bf.now, 2));
        assert_eq!(
            crate::facwar::occupation(&bf.reg, bf.system).map(|o| o.score),
            Some(vp),
            "once"
        );
    }

    #[test]
    fn the_slim_carries_the_score_for_board_items_only_and_zero_at_the_start() {
        let mut bf = battlefield(false);
        let got = slim_extras(&bf.reg, &[bf.item]);
        assert_eq!(got, vec![(bf.item, vec![(KEY_SCORE, PyValue::Int(0))])]);
        assert!(slim_extras(&bf.reg, &[bf.item + 1, 12]).is_empty());
        credit(&mut bf.reg, bf.site, fx::B, 4, at(bf.now, 1));
        let got = slim_extras(&bf.reg, &[bf.item, 12]);
        assert_eq!(got, vec![(bf.item, vec![(KEY_SCORE, PyValue::Int(4))])]);
    }

    #[test]
    fn a_saved_row_survives_a_restart_and_rebinds_to_the_new_item() {
        let mut bf = battlefield(false);
        credit(&mut bf.reg, bf.site, fx::B, 5, at(bf.now, 1));
        // A restart: the memory is gone, the delta rows stay.
        *bf.reg.ext_mut::<State>() = State::default();
        tick(&mut bf.reg, at(bf.now, 2));
        assert_eq!(bf.score(), 5, "read back from the fwsb row");
        // The instance is laid out again with other ids.
        assert!(crate::site::vacate(&mut bf.reg, bf.site));
        tick(&mut bf.reg, at(bf.now, 3));
        assert!(bf.reg.ext::<State>().board(bf.site).is_none(), "site gone");
        let site = fx::fw_sites(&bf.reg, bf.system)
            .into_iter()
            .find(|s| s.site_id == bf.site)
            .expect("the slot is still live");
        assert!(crate::site::instantiate(&mut bf.reg, &site));
        let sb = crate::site::add_object(&mut bf.reg, bf.site, SB_TYPE, bf.anchor).expect("placed");
        assert_ne!(sb.item_id, bf.item, "a new instance has new item ids");
        tick(&mut bf.reg, at(bf.now, 4));
        let b = bf
            .reg
            .ext::<State>()
            .board(bf.site)
            .cloned()
            .expect("board");
        assert_eq!((b.score, b.item_id), (5, Some(sb.item_id)));
    }

    #[test]
    fn a_system_that_changed_hands_negates_the_score() {
        let mut bf = battlefield(false);
        credit(&mut bf.reg, bf.site, fx::B, 5, at(bf.now, 1));
        // B takes S1: the sides swap (the fwvp row `score occupier taken_at credited`).
        bf.reg.deltas_mut().put(
            crate::facwar::KIND,
            vec![bf.system.to_string()],
            vec!["0".into(), fx::B.to_string(), String::new(), String::new()],
        );
        tick(&mut bf.reg, at(bf.now, 2));
        assert_eq!(bf.score(), -5, "the same contest seen from the other side");
        let b = bf
            .reg
            .ext::<State>()
            .board(bf.site)
            .cloned()
            .expect("board");
        assert_eq!((b.attacker, b.occupier), (fx::A, fx::B));
        assert_eq!(
            saved(&bf.reg, bf.system, bf.site).map(|s| (s.0, s.1)),
            Some((-5, fx::B))
        );
    }

    fn kill_of(
        victim: i64,
        killer: i64,
        ship_type: i64,
        when: BlueTime,
    ) -> crate::kill_hooks::KillEvent {
        crate::kill_hooks::KillEvent {
            victim_character: victim,
            victim_ship: victim + 100,
            ship_type,
            killer: Some(killer),
            system: fx::S1,
            npc: None,
            when,
            park: Vec::new(),
        }
    }

    /// An invented hull type that is not the capsule.
    const HULL: i64 = 990_555;

    #[test]
    fn a_militia_kill_on_the_grid_scores_for_the_killers_side() {
        let mut bf = battlefield(false);
        crate::service::facwar_mgr::enlist(&mut bf.reg, fx::YOU, fx::A, bf.now)
            .expect("YOU enlists");
        bf.put(fx::YOU, bf.anchor);
        on_kill(&mut bf.reg, &kill_of(fx::YOU, fx::ME, HULL, at(bf.now, 1)));
        assert_eq!(bf.score(), KILL_POINTS, "the attacker's kill adds");
        on_kill(&mut bf.reg, &kill_of(fx::ME, fx::YOU, HULL, at(bf.now, 1)));
        assert_eq!(bf.score(), 0, "the occupier's kill subtracts");
    }

    #[test]
    fn off_grid_non_militia_and_capsule_kills_score_nothing() {
        let mut bf = battlefield(false);
        let far = [
            bf.anchor[0] + 10.0 * crate::site::ARRIVAL_RANGE,
            bf.anchor[1],
            bf.anchor[2],
        ];
        // The victim is in no militia.
        bf.put(fx::YOU, bf.anchor);
        on_kill(&mut bf.reg, &kill_of(fx::YOU, fx::ME, HULL, at(bf.now, 1)));
        assert_eq!(bf.score(), 0, "a victim outside the militias");
        crate::service::facwar_mgr::enlist(&mut bf.reg, fx::YOU, fx::A, bf.now)
            .expect("YOU enlists");
        // A capsule.
        on_kill(
            &mut bf.reg,
            &kill_of(
                fx::YOU,
                fx::ME,
                crate::damage::CAPSULE_TYPE_ID,
                at(bf.now, 1),
            ),
        );
        assert_eq!(bf.score(), 0, "a capsule");
        // The killer is off the board's grid.
        bf.put(fx::ME, far);
        on_kill(&mut bf.reg, &kill_of(fx::YOU, fx::ME, HULL, at(bf.now, 1)));
        assert_eq!(bf.score(), 0, "off grid");
        // Back on it: it scores, so the three above were the guards at work.
        bf.put(fx::ME, bf.anchor);
        on_kill(&mut bf.reg, &kill_of(fx::YOU, fx::ME, HULL, at(bf.now, 1)));
        assert_eq!(bf.score(), KILL_POINTS);
    }

    #[test]
    fn the_rollover_drops_the_rows_of_gone_sites() {
        let mut bf = battlefield(false);
        credit(&mut bf.reg, bf.site, fx::B, 5, at(bf.now, 1));
        let gone = vec![fx::S1.to_string(), "900001".to_owned()];
        bf.reg.deltas_mut().put(
            KIND,
            gone.clone(),
            vec!["1".into(), fx::A.to_string(), String::new(), String::new()],
        );
        assert_eq!(bf.reg.deltas().of(KIND).len(), 2);
        assert_eq!(on_rollover(&mut bf.reg, 1), 1);
        assert!(bf.reg.deltas().of(KIND).get(&gone).is_none());
        assert!(
            saved(&bf.reg, bf.system, bf.site).is_some(),
            "the live one stays"
        );
    }

    #[test]
    fn the_gm_lever_lists_adds_and_resets() {
        let mut bf = battlefield(false);
        let text = |v: PyValue| match v {
            PyValue::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
            other => format!("{other:?}"),
        };
        let got = text(slash(&mut bf.reg, Some(fx::ME), &["list"]).expect("answers"));
        assert!(got.contains(&format!("site {}", bf.site)), "{got}");
        assert!(got.contains("wins 30") && got.contains("wins 40"), "{got}");
        let got = text(slash(&mut bf.reg, Some(fx::ME), &["add", "7"]).expect("answers"));
        assert!(got.contains("score is now 7"), "{got}");
        assert_eq!(bf.score(), 7, "toward the caller's militia");
        let got = text(slash(&mut bf.reg, Some(fx::ME), &["reset"]).expect("answers"));
        assert!(got.contains("1 battlefield"), "{got}");
        assert_eq!(bf.score(), 0);
        let got = text(slash(&mut bf.reg, Some(fx::YOU), &["add", "1"]).expect("answers"));
        assert!(got.contains("not in a militia"), "{got}");
    }

    #[test]
    fn the_list_names_battlefield_slots_that_have_no_board_yet() {
        let mut reg = world();
        let site = fx::fw_sites(&reg, fx::S1)
            .into_iter()
            .find(|s| s.archetype == BATTLEFIELD)
            .expect("a battlefield slot in S1");
        let got = match slash(&mut reg, Some(fx::ME), &["list"]).expect("answers") {
            PyValue::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
            other => format!("{other:?}"),
        };
        assert!(
            got.contains(&format!(
                "battlefield slot site {} system {} (no board yet)",
                site.site_id,
                fx::S1
            )),
            "{got}"
        );
    }

    /// FW-7: a looping point at a battlefield moves the board, not the
    /// system's VP (a complex's point still moves the VP: `fw_capture`'s
    /// multi-point test).
    #[test]
    fn a_battlefield_point_scores_the_board_and_not_the_vp() {
        let mut bf = battlefield(true);
        let cycle = crate::fw_capture::assumed().seconds * 10_000_000;
        let step = crate::fw_capture::CHECK_EVERY;
        let mut t = bf.now;
        while t < bf.now + 2 * cycle {
            crate::fw_capture::tick(&mut bf.reg, t);
            t += step;
        }
        assert!(bf.score() >= 1, "the held point scored the board");
        assert_eq!(
            crate::facwar::occupation(&bf.reg, bf.system).map(|o| o.score),
            Some(0),
            "and not the system's victory points"
        );
    }

    #[test]
    fn a_taken_point_at_a_battlefield_scores_and_never_completes_the_slot() {
        let mut bf = battlefield(true);
        bf.reg
            .ext_mut::<crate::facwar::Facwar>()
            .capture_mut()
            .set_params(crate::fw_capture::Params {
                looping: false,
                ..crate::fw_capture::assumed()
            });
        let cycle = crate::fw_capture::assumed().seconds * 10_000_000;
        let step = crate::fw_capture::CHECK_EVERY;
        let mut t = bf.now;
        while t < bf.now + 3 * cycle {
            crate::fw_capture::tick(&mut bf.reg, t);
            t += step;
        }
        assert!(
            bf.reg
                .ext::<crate::facwar::Facwar>()
                .capture()
                .is_captured(bf.site),
            "the point was taken"
        );
        assert_eq!(
            bf.score(),
            crate::fw_capture::assumed().point_value,
            "scored once, by the point's value"
        );
        let state = population::sites(&bf.reg, bf.system, bf.now)
            .sites
            .into_iter()
            .find(|s| s.site_id == bf.site)
            .map(|s| s.state);
        assert_eq!(state, Some(population::State::Live), "not completed");
    }
}
