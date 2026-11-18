//! What hears an item-trader trade (CQ-2, issue #291; CQ-4 fills it).
//!
//! [`on_trade`] is called by [`crate::item_trader::trade`] after a trade has
//! happened in full, with the trader's ball id, the recipe and the
//! multiplier. It has no hooks yet: CQ-4 adds the turn-in counting here.

use crate::service::Registry;

/// A good trade of `multiplier` times `recipe` at `trader` by `actor` has
/// been made. No hook yet.
pub fn on_trade(_w: &mut Registry, _actor: i64, _trader: i64, _recipe: i64, _multiplier: i64) {}
