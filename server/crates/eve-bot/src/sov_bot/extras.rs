//! **The sov extras** (SOVX, #221): the steward, raider and den-runner duties
//! that sit beside the campaign duties of [`super`]. This module is the seam:
//! the dispatch hook ([`turn`]), the gateway seam ([`gw`]) and the sov kit on
//! the builders' ladder ([`kit`]). The duties themselves come in later lanes.
//!
//! **The rule.** Every act is a client verb: a drop from the ship, a ballpark
//! component call, a gateway request a player's client also sends. No slash
//! line, no GM tool. Items come only from the market or the pilot's own
//! hangar (nothing from nowhere); a pilot that lacks one says `MISSING <what>`.
//!
//! **The duties and who gets them.** The steward (B1-a) is a
//! [`Duty::Haul`] pilot; the raider (B2) is a [`Duty::Idle`] pilot; the den
//! runner (B3) is a [`Duty::Haul`] pilot with the den skill. Each later lane
//! adds one arm to [`ARMS`] and its own sub-struct to [`Mind`].
//!
//! **The clock.** Every gateway time is wall unix seconds
//! (`clock::wall_unix_secs()`); every pause of this module is game time
//! (`crate::clock`), as the rest of the bots.
//!
//! **[A] B0-1**: [`EXTRAS_EVERY`] is 60 s between passes per pilot. Falsified
//! by a soak where a steward's notes show it idle while a hub runs dry, or by
//! gateway request counts that dominate the server's turn time.

pub mod gw;
pub mod kit;
pub mod steward;
#[cfg(test)]
mod steward_tests;
#[cfg(test)]
mod tests;
pub mod upgrades;
#[cfg(test)]
mod upgrades_tests;

use super::{Ctx, Duty, Plan};
use crate::brain::Memory;
use crate::clock;
use crate::{Conn, Error, Result};
use std::time::{Duration, Instant};

/// How long the extras leave a pilot alone between two passes (**[A] B0-1**).
pub const EXTRAS_EVERY: Duration = Duration::from_secs(60);

/// What a pilot remembers for the extras (later lanes add their own
/// sub-structs, each `Default`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mind {
    /// The next pass is not before this.
    pub next: Option<Instant>,
    /// When the gateway was last found missing ([`gw::GW_BACKOFF`]).
    pub gw_down: Option<Instant>,
    /// The steward's memory (B1-a).
    pub steward: steward::Mind,
}

/// One duty: `Ok(None)` is nothing to do this pass.
type Arm = fn(&mut Conn, &mut Memory, &Ctx<'_>, &Plan, Duty) -> Result<Option<String>>;

/// The duties, in order; the first that answers a line wins the turn.
const ARMS: &[Arm] = &[steward::arm, upgrades::arm];

/// **The hook** `sov_bot::turn` calls before the duty match: `Ok(None)` means
/// the old duty runs unchanged. A dead connection (`Io`, `Protocol`) is the
/// error; any other error is a note, as everywhere in `sov_bot`.
pub fn turn(
    c: &mut Conn,
    m: &mut Memory,
    ctx: &Ctx<'_>,
    plan: &Plan,
    duty: Duty,
) -> Result<Option<String>> {
    let now = clock::instant();
    if m.sov.extras.next.is_some_and(|t| now < t) {
        return Ok(None);
    }
    m.sov.extras.next = Some(now + EXTRAS_EVERY);
    for arm in ARMS {
        match arm(c, m, ctx, plan, duty) {
            Ok(None) => {}
            Ok(line) => return Ok(line),
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(e) => return Ok(Some(format!("sov extras: {e}"))),
        }
    }
    Ok(None)
}
