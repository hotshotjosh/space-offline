//! **The sov kit on the builders' ladder**: skyhooks, hub upgrades and
//! mercenary dens are built by the builders, and a builder climbs for one only
//! while a steward has said it wants one (the want board). The rungs go last
//! on the ladder, so they never displace a career rung; the built product is
//! promoted and sold by the existing paths.
//!
//! **[A] B0-3**: [`WANT_TTL`] is 6 h. Falsified by builders holding a kit
//! original nobody bids for (too long), or a steward whose bid stands while no
//! builder climbs the rung (too short).
//!
//! **[A] B0-5**: stewards and builders run in one `eve-bots` process (the board
//! is per process, like `SovHall`). Falsified by a soak with split processes
//! where stewards bid and no builder climbs.

use crate::clock;
use eve_server::content::Content;
use eve_server::industry;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a want stands (**[A] B0-3**).
pub const WANT_TTL: Duration = Duration::from_secs(6 * 3600);

static WANTS: Mutex<BTreeMap<i64, Instant>> = Mutex::new(BTreeMap::new());

/// Whether a type group is part of the sov kit: skyhooks, the four upgrade
/// groups and mercenary dens.
pub fn is_kit(group: i64) -> bool {
    group == eve_server::skyhook::GROUP_SKYHOOK
        || eve_server::sov_upgrade::UPGRADE_GROUPS.contains(&group)
        || group == eve_server::merc_den::GROUP_MERCENARY_DEN
}

/// A steward says it wants `product` (a type id), now.
pub fn want(product: i64) {
    WANTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(product, clock::instant());
}

/// The products wanted at `now`: stamped within [`WANT_TTL`].
pub fn wanted(now: Instant) -> BTreeSet<i64> {
    WANTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(_, at)| now.saturating_duration_since(**at) < WANT_TTL)
        .map(|(p, _)| *p)
        .collect()
}

/// **The pure core**: one rung per wanted kit product, each the sorted, deduped
/// manufacturing originals that make it. A product the item store cannot
/// create is left out (the sov hub rung's rule: a job for an unknown product
/// is refused by the server). Empty when nothing is wanted.
pub fn kit_rungs(
    recipes: &industry::Table,
    group_of: &dyn Fn(i64) -> Option<i64>,
    creatable: &dyn Fn(i64) -> bool,
    wanted: &BTreeSet<i64>,
) -> Vec<Vec<i64>> {
    let mut by_product: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for bp in &recipes.blueprints {
        let product = bp.product_type_id;
        if wanted.contains(&product) && group_of(product).is_some_and(is_kit) && creatable(product)
        {
            by_product
                .entry(product)
                .or_default()
                .push(bp.blueprint_type_id);
        }
    }
    by_product
        .into_values()
        .map(|mut r| {
            r.sort_unstable();
            r.dedup();
            r
        })
        .collect()
}

/// The kit rungs for the products wanted now, over the type data.
pub fn rungs(content: &Content, recipes: &industry::Table) -> Vec<Vec<i64>> {
    kit_rungs(
        recipes,
        &|t| crate::hulls::group_of(content, t),
        &|t| eve_server::item::find_in(content, t).is_some(),
        &wanted(clock::instant()),
    )
}
