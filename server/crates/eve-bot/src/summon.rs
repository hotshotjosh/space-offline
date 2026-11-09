//! **Coming to a summoner** (`/bots come`, #162): the pure decisions.
//!
//! A bot that is called warps to the caller's character. A cloaked caller's
//! ship is not on the bot's world, and neither is a caller on another grid,
//! so the bot cannot tell them apart; the old turn warped to the caller at
//! 2500 m every 20 s. A landing within `cloak::MIN_CLOAKING_DISTANCE`
//! (surface to surface) decloaks him.
//!
//! The rules now: **every** summon warp asks for a range that clears the
//! cloak distance by the bot's own radius and the caller's; the server's
//! "already within the minimum warp distance" refusal (a caller on the
//! bot's grid it cannot see) settles the summon -- no more warps until the
//! bot's grid changes -- and any other refusal stays a failure counted
//! against the tries limit. The follow, `SUMMON_HOLD_S`, is untouched: a
//! caller who warps on is still followed.

use eve_server::cloak::MIN_CLOAKING_DISTANCE;

/// Metres added for the radius of a caller the bot cannot see **[A]**.
/// Falsified by a decloak caused by a bot landing next to a hull larger
/// than this.
pub const UNSEEN_CALLER_RADIUS: f64 = 1_000.0;

/// How long a settled summon stays settled **[A]**. Falsified by a bot that
/// sits for minutes beside a caller who has left, or one that re-warps to a
/// cloaked caller often enough to be noticed. A retry is always safe: every
/// warp clears the cloak distance.
pub const SETTLE_HOLD: std::time::Duration = std::time::Duration::from_secs(60);

/// Whether a settled summon still holds: the bot's grid is the one it
/// settled on, the caller is still not visible, and the hold has not run out.
pub fn settle_holds(same_grid: bool, age: std::time::Duration, caller_visible: bool) -> bool {
    same_grid && !caller_visible && age < SETTLE_HOLD
}

/// What one summon turn does about the warp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// Nothing this turn (the role's turn runs).
    Hold,
    /// Warp to the caller's character at this minimum range.
    Warp { range: f64 },
}

/// What the turn knows.
#[derive(Debug, Clone, Copy)]
pub struct View {
    /// The caller's ship is on the bot's world and near.
    pub near: bool,
    /// The bot's own warp is running.
    pub warping: bool,
    /// The repeat interval has run since the last warp (or none was made).
    pub due: bool,
    /// The server said the unseen caller is within the minimum warp distance.
    pub settled: bool,
    /// The range the caller asked for.
    pub requested: f64,
    /// The bot's own radius.
    pub own_radius: f64,
    /// The caller's radius when his ship is on the bot's world.
    pub caller_radius: Option<f64>,
}

/// The range for any summon warp: clear of the cloak distance, surface to
/// surface, whatever the hulls (the server lands the bot at the larger of
/// this and the target's radius plus a standoff, centre to centre).
pub fn range(requested: f64, own_radius: f64, caller_radius: Option<f64>) -> f64 {
    let floor = MIN_CLOAKING_DISTANCE + own_radius + caller_radius.unwrap_or(UNSEEN_CALLER_RADIUS);
    requested.max(floor)
}

/// The step for this turn.
pub fn step(v: View) -> Step {
    if v.warping || v.settled || v.near || !v.due {
        return Step::Hold;
    }
    Step::Warp {
        range: range(v.requested, v.own_radius, v.caller_radius),
    }
}

/// The words of the server's minimum-warp-distance refusal
/// (`service/space.rs` `too_short`) **[A]**: falsified by that text changing,
/// which makes the bot retry every interval instead of settling (safe).
const TOO_SHORT_WORDS: &str = "closer than 150 km";

/// The `notify` text of a `CustomNotify` error frame, if any. `CustomNotify`
/// is shared (entosis `linking`, the doomsday immobile guard), so the key
/// alone does not say which refusal it was.
pub fn notify_text(result: &eve_marshal::PyValue) -> Option<String> {
    use eve_marshal::PyValue;
    let packet = eve_proto::MachoPacket::from_value(result).ok()?;
    packet.body.iter().find_map(|part| {
        let PyValue::Tuple(wrapped) = part else {
            return None;
        };
        let Some(PyValue::Bytes(pickle)) = wrapped.first() else {
            return None;
        };
        let Ok(PyValue::ObjectEx { header, .. }) = eve_server::loads(pickle) else {
            return None;
        };
        let PyValue::Tuple(header) = header.as_ref() else {
            return None;
        };
        let Some(PyValue::Tuple(args)) = header.get(1) else {
            return None;
        };
        args.iter().find_map(|a| match a {
            // Pairs are (value, key) in this marshal's dicts.
            PyValue::Dict(d) => d.iter().find_map(|(v, _)| match v {
                PyValue::Bytes(t) => Some(String::from_utf8_lossy(t).into_owned()),
                _ => None,
            }),
            _ => None,
        })
    })
}

/// Whether a refusal (the driver's text: the call, the key and, for a
/// `CustomNotify`, its words) is the server's minimum-warp-distance one and
/// not a scramble, a bubble or another `CustomNotify`.
pub fn says_already_near(refusal: Option<&str>) -> bool {
    refusal.is_some_and(|w| {
        w.contains("CustomNotify")
            && w.contains(TOO_SHORT_WORDS)
            && !crate::scram::is_refusal(w)
            && !crate::warp_bubble::is_refusal(w)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View {
            near: false,
            warping: false,
            due: true,
            settled: false,
            requested: 100.0,
            own_radius: 40.0,
            caller_radius: None,
        }
    }

    #[test]
    fn every_warp_clears_the_cloak_distance_by_the_hull_radii() {
        let Step::Warp { range } = step(view()) else {
            panic!("warps");
        };
        assert!(range >= MIN_CLOAKING_DISTANCE + 40.0);
        let mut v = view();
        v.caller_radius = Some(250.0);
        assert_eq!(
            step(v),
            Step::Warp {
                range: MIN_CLOAKING_DISTANCE + 40.0 + 250.0
            }
        );
    }

    #[test]
    fn the_old_visible_range_is_raised_to_the_floor_and_a_bigger_request_stands() {
        let mut v = view();
        v.requested = 2_500.0;
        v.own_radius = 300.0;
        v.caller_radius = Some(300.0);
        let Step::Warp { range } = step(v) else {
            panic!("warps");
        };
        assert!(range >= MIN_CLOAKING_DISTANCE + 600.0);
        v.requested = 50_000.0;
        assert_eq!(step(v), Step::Warp { range: 50_000.0 });
    }

    #[test]
    fn a_caller_who_moves_grid_keeps_the_follow() {
        // Not on the bot's world (he warped on), not settled: the bot warps
        // again once the interval has run.
        let mut v = view();
        v.due = false;
        assert_eq!(step(v), Step::Hold);
        v.due = true;
        assert!(matches!(step(v), Step::Warp { .. }));
    }

    #[test]
    fn a_settled_summon_holds_until_it_is_unsettled() {
        let mut v = view();
        v.settled = true;
        assert_eq!(step(v), Step::Hold);
        v.settled = false;
        assert!(matches!(step(v), Step::Warp { .. }));
    }

    #[test]
    fn a_running_warp_or_a_near_visible_caller_holds() {
        let mut v = view();
        v.warping = true;
        assert_eq!(step(v), Step::Hold);
        v.warping = false;
        v.near = true;
        assert_eq!(step(v), Step::Hold);
    }

    #[test]
    fn only_the_too_short_refusal_means_already_near() {
        let short = "CmdWarpToStuff('char', 7): CustomNotify [You cannot warp to a destination closer than 150 km.]";
        assert!(says_already_near(Some(short)));
        assert!(!says_already_near(None));
        assert!(!says_already_near(Some("CmdWarpToStuff('char', 7): Other")));
        // The other CustomNotify refusals (invented words) are not it.
        for words in ["Your link is active.", "You are held in place."] {
            let w = format!("CmdWarpToStuff('char', 7): CustomNotify [{words}]");
            assert!(!says_already_near(Some(&w)));
        }
        assert!(!says_already_near(Some(
            "CmdWarpToStuff('char', 7): CustomNotify"
        )));
    }

    #[test]
    fn the_notify_words_are_read_out_of_the_frame() {
        let frame = eve_proto::MachoPacket::error_response(
            eve_proto::MachoAddress::node(1),
            eve_proto::MachoAddress::client(1),
            eve_marshal::MachoNetMsg::CallReq,
            2,
            eve_proto::wrapped_exception_payload(&eve_proto::user_error(
                "CustomNotify",
                vec![(
                    eve_marshal::PyValue::Bytes(b"a destination closer than 150 km".to_vec()),
                    eve_marshal::PyValue::Bytes(b"notify".to_vec()),
                )],
            )),
        )
        .to_value()
        .expect("encodes");
        assert_eq!(
            notify_text(&frame).as_deref(),
            Some("a destination closer than 150 km")
        );
        assert_eq!(notify_text(&eve_marshal::PyValue::None), None);
    }

    #[test]
    fn a_settle_ends_when_the_caller_shows_up_or_the_hold_runs_out() {
        use std::time::Duration;
        let young = Duration::from_secs(1);
        assert!(settle_holds(true, young, false));
        // Visible caller 50 km away on the bot's grid: follow him.
        assert!(!settle_holds(true, young, true));
        // A cloaked caller who warped off: the hold expires.
        assert!(!settle_holds(true, SETTLE_HOLD, false));
        // The bot's own grid changed.
        assert!(!settle_holds(false, young, false));
    }
}
