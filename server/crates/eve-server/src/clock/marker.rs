//! The world marker codec and its refusal table (#147 FF-1, spec
//! `fast-forward.md` §2.5). A fast-forward world carries `ffclock {epoch,
//! mode}` so that the normal server, whose stamps are real, refuses to load
//! it, and the fast-forward host refuses a world that was never marked.
//!
//! **No write path yet.** `Deltas::put` debug-asserts that its kind is listed
//! in `delta_kinds.rs`, and that file is hot; the row `("ffclock", "clock")`
//! is a later maintainer line. Until then this module is only the codec and
//! the decision. Hosted by `clock.rs` through an explicit `#[path]`.

/// The delta kind of the marker (`delta_kinds.rs` row pending).
pub const KIND: &str = "ffclock";

/// What a fast-forward world records about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    /// The virtual epoch, whole Unix seconds, the world began at.
    pub epoch_unix: u64,
    /// The clock mode that wrote it (`"affine"` or `"lockstep"`; free text
    /// to the codec).
    pub mode: String,
}

impl Marker {
    /// The delta value fields: `[epoch_unix, mode]`.
    pub fn encode(&self) -> Vec<String> {
        vec![self.epoch_unix.to_string(), self.mode.clone()]
    }

    /// Read the fields back. `None` for a wrong count, a non-numeric epoch
    /// or an empty mode: a damaged marker is not a marker.
    pub fn decode(fields: &[String]) -> Option<Marker> {
        let [epoch, mode] = fields else {
            return None;
        };
        if mode.is_empty() {
            return None;
        }
        let epoch_unix: u64 = epoch.parse().ok()?;
        // An epoch past what `SystemTime` can hold would panic once it is an
        // Affine origin: such a marker is not one.
        std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(epoch_unix))?;
        Some(Marker {
            epoch_unix,
            mode: mode.clone(),
        })
    }
}

/// The decision table of spec §2.5. `found` is the marker the world carries
/// (if any), `host` whether the caller is the fast-forward host, `adopt`
/// whether `--ff-adopt` was given.
///
/// | caller | world | result |
/// |---|---|---|
/// | normal server | marked | refuse |
/// | normal server | unmarked | ok |
/// | host | marked | ok |
/// | host | unmarked, no adopt | refuse |
/// | host | unmarked, adopt | ok (the host then writes the marker) |
pub fn refuse_marked(found: Option<&Marker>, host: bool, adopt: bool) -> Result<(), String> {
    match (host, found) {
        (false, Some(m)) => Err(format!(
            "this world is a fast-forward world (ffclock epoch {} mode {}): its stamps are \
             virtual, so the normal server will not load it",
            m.epoch_unix, m.mode
        )),
        (true, None) if !adopt => Err(
            "this world was never run fast-forward; pass --ff-adopt to adopt a throwaway world"
                .to_owned(),
        ),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m() -> Marker {
        Marker {
            epoch_unix: 1_700_000_123,
            mode: "affine".to_owned(),
        }
    }

    #[test]
    fn an_epoch_past_system_time_is_not_a_marker() {
        let f = [u64::MAX.to_string(), "affine".to_owned()];
        assert_eq!(Marker::decode(&f), None);
    }

    #[test]
    fn round_trip() {
        assert_eq!(Marker::decode(&m().encode()), Some(m()));
        assert_eq!(
            m().encode(),
            vec!["1700000123".to_owned(), "affine".to_owned()]
        );
    }

    #[test]
    fn decode_rejects_damage() {
        let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        assert_eq!(Marker::decode(&[]), None);
        assert_eq!(Marker::decode(&s(&["5"])), None);
        assert_eq!(Marker::decode(&s(&["5", "a", "b"])), None);
        assert_eq!(Marker::decode(&s(&["x", "affine"])), None);
        assert_eq!(Marker::decode(&s(&["-1", "affine"])), None);
        assert_eq!(Marker::decode(&s(&["5", ""])), None);
    }

    #[test]
    fn the_normal_server_refuses_a_marked_world() {
        // adopt means nothing to the normal server
        assert!(refuse_marked(Some(&m()), false, false).is_err());
        assert!(refuse_marked(Some(&m()), false, true).is_err());
        assert!(refuse_marked(None, false, false).is_ok());
        assert!(refuse_marked(None, false, true).is_ok());
    }

    #[test]
    fn the_host_refuses_an_unmarked_world_without_adopt() {
        assert!(refuse_marked(None, true, false).is_err());
        assert!(refuse_marked(None, true, true).is_ok());
        assert!(refuse_marked(Some(&m()), true, false).is_ok());
        assert!(refuse_marked(Some(&m()), true, true).is_ok());
    }
}
