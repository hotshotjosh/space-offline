//! Protobuf helpers for the sovereignty gateway arms (SOV-GW-0). `gateway.rs`
//! keeps `put_uint` and `timestamp` private, so the sov lanes share these.
//! Wire contract: `docs/re/sov-gateway.md`.

// The later sov lanes (L3-L11) call the helpers not used yet.
#![allow(dead_code)]

use crate::gateway::{Reply, get_bytes, get_uint, put_varint};

/// A varint field (wire type 0).
pub(crate) fn put_uint(out: &mut Vec<u8>, field: u64, v: u64) {
    put_varint(out, field << 3);
    put_varint(out, v);
}

/// A bool field. Always writes the tag, so a oneof arm set to `true` is on
/// the wire [V] (the workforce `inactive` arm is read by its presence).
pub(crate) fn put_bool(out: &mut Vec<u8>, field: u64, v: bool) {
    put_uint(out, field, u64::from(v));
}

/// A proto `int32`: a negative value is a ten-byte two's-complement varint.
pub(crate) fn put_int32(out: &mut Vec<u8>, field: u64, v: i32) {
    put_uint(out, field, i64::from(v) as u64);
}

/// `google.protobuf.Timestamp { 1 seconds }`; zero is written, not elided.
pub(crate) fn timestamp(secs: u64) -> Vec<u8> {
    let mut out = Vec::new();
    put_uint(&mut out, 1, secs);
    out
}

/// `google.protobuf.Duration { 1 seconds }`.
pub(crate) fn duration(secs: u64) -> Vec<u8> {
    timestamp(secs)
}

/// An identifier message `{ 1 sequential }`.
pub(crate) fn id(seq: u64) -> Vec<u8> {
    let mut out = Vec::new();
    put_uint(&mut out, 1, seq);
    out
}

/// The `sequential` of the identifier message in `field` of `body`. A
/// client-sent sequential of 0 is elided on the wire and decodes as `None`.
pub(crate) fn read_id(body: &[u8], field: u64) -> Option<u64> {
    get_uint(&get_bytes(body, field)?, 1)
}

/// Wall-clock unix seconds.
pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// 200 with the `<base>Response` of request type `ty`.
pub(crate) fn typed(ty: &str, value: Vec<u8>) -> Reply {
    let base = ty.strip_suffix("Request").unwrap_or(ty);
    Reply {
        status: 200,
        payload: Some((format!("{base}Response"), value)),
    }
}

/// A bare status, no payload.
pub(crate) fn status(code: u32) -> Reply {
    Reply {
        status: code,
        payload: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::put_bytes;

    #[test]
    fn negative_int32_is_ten_bytes_ending_one() {
        let mut out = Vec::new();
        put_int32(&mut out, 1, -1);
        assert_eq!(out.len(), 11, "tag + ten varint bytes");
        assert_eq!(*out.last().unwrap(), 0x01);
    }

    #[test]
    fn timestamp_zero_still_writes_the_field() {
        assert_eq!(timestamp(0), vec![0x08, 0x00]);
        assert_eq!(duration(5), vec![0x08, 0x05]);
    }

    #[test]
    fn read_id_round_trips() {
        let mut body = Vec::new();
        put_bytes(&mut body, 3, &id(42));
        assert_eq!(read_id(&body, 3), Some(42));
        assert_eq!(read_id(&body, 2), None);
    }

    #[test]
    fn bool_true_and_false_both_write_their_tag() {
        let mut a = Vec::new();
        put_bool(&mut a, 2, true);
        let mut b = Vec::new();
        put_bool(&mut b, 2, false);
        assert_eq!(a, vec![0x10, 1]);
        assert_eq!(b, vec![0x10, 0]);
    }

    #[test]
    fn typed_names_the_response() {
        let r = typed("eve_public.x.api.GetRequest", vec![1]);
        assert_eq!(r.status, 200);
        assert_eq!(r.payload.unwrap().0, "eve_public.x.api.GetResponse");
        assert_eq!(status(403).payload, None);
    }
}
