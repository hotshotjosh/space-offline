//! **Character portraits from the creator's own doll** (#205).
//!
//! A character's face is the paperdoll the character creator built. On
//! Tranquility the image server renders it; this server runs none, so a
//! portrait comes from the client's **scene portrait** path instead: when the
//! image-server picture is missing and the session holds `ROLE_PROGRAMMER`
//! (which ours do, `docs/reference/session-roles.md`), `EvePhoto.GetPortrait`
//! queues a scene render, and `ScenePortraitQueue._FetchBatchData` asks
//! `paperDollServer.GetPaperDollDataBatch` for the DNA and
//! `GetPaperDollPortraitDataFor` for the camera and light **[V decompiled
//! `evephotosvc/scenePortraitQueue.pyc`, `ccSvc.GetPortraitData`]**.
//!
//! Those were answered empty, so after a client restart every portrait was
//! the default head (the creator's own 512 px snapshot is only trusted in the
//! session that made it).
//!
//! This module keeps what the creator sends: `CreateCharacterWithDoll`'s
//! `charInfo` (a KeyVal of `colors`, `modifiers`, `sculpts`, `appearance`:
//! the fields of `paperdoll_dna.PaperdollDNA` **[V decompiled]**) and
//! `portraitInfo` (camera, light, pose, background), each stored as one
//! marshal stream in delta kind [`KIND`]. They are handed back verbatim:
//! the client built them, so the client can read them.
//!
//! **[A]** handing `charInfo` back as the DNA. Falsified by a scene render
//! that fails on it (the client logs the render error and keeps the default
//! head).

use crate::service::Registry;
use eve_marshal::PyValue;

/// The delta kind (`delta_kinds::KINDS`, owner `paperdoll`):
/// `char -> charInfo portraitInfo`, each a hex marshal stream.
pub const KIND: &str = "pdoll";

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Keep the creator's doll of `ch`: `char_info` and `portrait_info` as the
/// client sent them.
pub fn store(reg: &mut Registry, ch: i64, char_info: &PyValue, portrait_info: &PyValue) {
    let ci = to_hex(&crate::wire::dumps(char_info));
    let pi = to_hex(&crate::wire::dumps(portrait_info));
    reg.deltas_mut()
        .put(KIND, vec![ch.to_string()], vec![ci, pi]);
}

fn field(reg: &Registry, ch: i64, i: usize) -> Option<PyValue> {
    let row = reg.deltas().of(KIND).get(&[ch.to_string()][..])?;
    let bytes = from_hex(row.get(i)?)?;
    crate::wire::loads_unbudgeted(&bytes).ok()
}

/// `ch`'s DNA (`charInfo`), if the creator's doll was kept, with every
/// missing sculpt weight served as `0.0` ([`zero_sculpt_nones`]).
pub fn dna(reg: &Registry, ch: i64) -> Option<PyValue> {
    field(reg, ch, 0).map(zero_sculpt_nones)
}

fn is_key(k: &PyValue, name: &str) -> bool {
    match k {
        PyValue::Bytes(b) => b == name.as_bytes(),
        PyValue::Unicode(u) => u == name,
        _ => false,
    }
}

/// `None` -> `0.0` everywhere inside `v`.
fn zero_nones(v: PyValue) -> PyValue {
    match v {
        PyValue::None => PyValue::Real(0.0),
        PyValue::Tuple(xs) => PyValue::Tuple(xs.into_iter().map(zero_nones).collect()),
        PyValue::List(xs) => PyValue::List(xs.into_iter().map(zero_nones).collect()),
        PyValue::ObjectEx {
            form,
            header,
            list,
            dict,
        } => PyValue::ObjectEx {
            form,
            header: Box::new(zero_nones(*header)),
            list: list.into_iter().map(zero_nones).collect(),
            dict: dict.into_iter().map(|(k, x)| (k, zero_nones(x))).collect(),
        },
        PyValue::Instance { class, state } => PyValue::Instance {
            class,
            state: Box::new(zero_nones(*state)),
        },
        other => other,
    }
}

/// **A sculpt weight the creator left out is served as `0.0`.** The
/// creator sends `None` for a sculpt axis it does not use, but the doll
/// loader the character sheet runs (`character.ApplyDBRowToDoll` ->
/// `ConvertWeightKeyValue`) takes `abs()` of every weight and raised
/// `TypeError: bad operand type for abs(): 'NoneType'` on a dev char's doll
/// (`lashes_rightshape`) **[V client traceback, 19NOV2026]**. **[A]** a live
/// server stores 0 there: a zero weight is no morph on that axis, the same
/// as none. Only the `sculpts` entry is touched.
fn zero_sculpt_nones(v: PyValue) -> PyValue {
    match v {
        PyValue::Dict(pairs) => PyValue::Dict(
            pairs
                .into_iter()
                .map(|(val, key)| {
                    if is_key(&key, "sculpts") {
                        (zero_nones(val), key)
                    } else {
                        (zero_sculpt_nones(val), key)
                    }
                })
                .collect(),
        ),
        PyValue::ObjectEx {
            form,
            header,
            list,
            dict,
        } => PyValue::ObjectEx {
            form,
            header: Box::new(zero_sculpt_nones(*header)),
            list,
            dict: dict
                .into_iter()
                .map(|(k, x)| {
                    if is_key(&k, "sculpts") {
                        (k, zero_nones(x))
                    } else {
                        (k, zero_sculpt_nones(x))
                    }
                })
                .collect(),
        },
        PyValue::Instance { class, state } => PyValue::Instance {
            class,
            state: Box::new(zero_sculpt_nones(*state)),
        },
        PyValue::Tuple(xs) => PyValue::Tuple(xs.into_iter().map(zero_sculpt_nones).collect()),
        other => other,
    }
}

/// `ch`'s portrait setup (`portraitInfo`), if kept.
pub fn portrait_info(reg: &Registry, ch: i64) -> Option<PyValue> {
    field(reg, ch, 1)
}

/// `GetPaperDollDataBatch(charIDs)`: `{charID: dna}` for every id with a
/// kept doll. A real dict always (`_FetchBatchData` calls `iteritems`).
/// `PyValue::Dict` stores `(value, key)` (`pyutil::pydict`).
pub fn batch_value(reg: &Registry, ids: &[i64]) -> PyValue {
    PyValue::Dict(
        ids.iter()
            .filter_map(|&id| dna(reg, id).map(|d| (d, PyValue::Int(id))))
            .collect(),
    )
}

/// `GetPaperDollPortraitDataFor(charID)`: `(data, _)` where `data[0]` is the
/// portrait setup (`ccSvc.GetPortraitData` returns `data[0]` when
/// `len(data)`, else `None`).
pub fn portrait_value(reg: &Registry, ch: i64) -> PyValue {
    PyValue::Tuple(vec![
        PyValue::List(portrait_info(reg, ch).into_iter().collect()),
        PyValue::None,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(pairs: Vec<(&str, PyValue)>) -> PyValue {
        crate::pyutil::pydict(pairs)
    }

    /// A kept doll comes back unchanged through all three answers; a
    /// character without one gets the empty shapes.
    #[test]
    fn a_kept_doll_comes_back_verbatim() {
        let mut reg = crate::testkit::registry_with(|_| {});
        let ci = kv(vec![
            ("colors", PyValue::List(vec![PyValue::Int(1)])),
            ("sculpts", PyValue::List(vec![])),
        ]);
        let pi = kv(vec![
            ("lightID", PyValue::Int(10_866)),
            ("backgroundID", PyValue::Int(16)),
        ]);
        store(&mut reg, 91_000_001, &ci, &pi);
        assert_eq!(dna(&reg, 91_000_001), Some(ci.clone()));
        assert_eq!(portrait_info(&reg, 91_000_001), Some(pi.clone()));
        assert_eq!(
            batch_value(&reg, &[91_000_001, 91_000_002]),
            PyValue::Dict(vec![(ci, PyValue::Int(91_000_001))])
        );
        assert_eq!(
            portrait_value(&reg, 91_000_001),
            PyValue::Tuple(vec![PyValue::List(vec![pi]), PyValue::None])
        );
        assert_eq!(
            portrait_value(&reg, 91_000_002),
            PyValue::Tuple(vec![PyValue::List(vec![]), PyValue::None])
        );
        assert_eq!(dna(&reg, 91_000_002), None);
    }

    /// Only the sculpts' missing weights become `0.0`; `None` elsewhere stays.
    #[test]
    fn missing_sculpt_weights_are_served_as_zero() {
        let mut reg = crate::testkit::registry_with(|_| {});
        let sculpt = PyValue::Tuple(vec![PyValue::Int(7), PyValue::None, PyValue::Real(0.4)]);
        let ci = kv(vec![
            ("sculpts", PyValue::List(vec![sculpt])),
            ("appearance", PyValue::None),
        ]);
        store(&mut reg, 91_000_001, &ci, &PyValue::None);
        let want = kv(vec![
            (
                "sculpts",
                PyValue::List(vec![PyValue::Tuple(vec![
                    PyValue::Int(7),
                    PyValue::Real(0.0),
                    PyValue::Real(0.4),
                ])]),
            ),
            ("appearance", PyValue::None),
        ]);
        assert_eq!(dna(&reg, 91_000_001), Some(want));
    }

    #[test]
    fn hex_round_trips() {
        let b = vec![0u8, 1, 0x7e, 0xff];
        assert_eq!(from_hex(&to_hex(&b)), Some(b));
        assert_eq!(from_hex("abc"), None);
        assert_eq!(from_hex("zz"), None);
    }
}
