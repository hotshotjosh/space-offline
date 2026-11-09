//! **Abyss cache loot**: a Triglavian cache or node of an abyssal room that
//! dies leaves what the owner's table `content/pve/abyss-loot.tsv` draws in
//! its wreck. The owner's table, the core's rule: the rows, the draw and the
//! items are `eve_server::abyss::loot`'s; this module is the kill-hook row
//! that wires them in, as `drops` does for PLEX.
//!
//! No `settled` row: a pocket kill is never settled offline **[I]** (the
//! pilot is in space).

use eve_server::module::KillEvent;
use eve_server::service::Registry;

/// The kill-hook row: an NPC that is a cache of a run's room fills its wreck.
/// Runs after the wreck is made (`kill_hooks::NPC`); any other NPC is
/// ignored.
pub fn on_npc_kill(reg: &mut Registry, ev: &KillEvent) {
    eve_server::abyss::loot::on_npc_kill(reg, ev);
}
