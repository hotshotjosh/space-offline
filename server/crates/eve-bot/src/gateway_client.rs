//! A bot that speaks the public gateway: the same `eve_public.Request` a player's
//! client sends over h2c (HTTP/2 cleartext) to `/eve_public.gateway.Requests/Send`.
//!
//! Upgrades, the reagent bay, skyhook on/off and dens are gateway-only for the
//! client, so a bot that does them must send the same request a player sends;
//! anything else is a back door (L13 BOT-GW, issue #220).
//!
//! Std-only and one request at a time: connection preface, an empty SETTINGS, ACK
//! of the server's SETTINGS, then one stream per request. The request headers are
//! HPACK literals without indexing and without Huffman coding, which is what the
//! server's decoder reads (`eve-server/src/gateway.rs`, HPACK section) **[V]**.
//! The message layout is what `gateway.rs` `request_character` and `respond`
//! read: `Request` {2 correlation, 3 Any{1 type_url, 2 value},
//! 6 authoritative_context{10 identity{1 character{1 sequential}}}} and
//! `Response` {2 correlation, 3 status, 5 Any} **[V]**.

use crate::clock;
use crate::{Error, Result};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
const PATH: &str = "/eve_public.gateway.Requests/Send";
/// Our largest accepted frame (as the server's): a bigger one is a protocol fault.
const MAX_FRAME: usize = 1 << 20;
/// The most response data we collect for one request.
const MAX_RESPONSE: usize = 4 << 20;
/// The HTTP/2 default maximum frame size: our DATA frames stay within it.
const CHUNK: usize = 16_384;

const DATA: u8 = 0;
const HEADERS: u8 = 1;
const RST_STREAM: u8 = 3;
const SETTINGS: u8 = 4;
const PING: u8 = 6;
const WINDOW_UPDATE: u8 = 8;
const GOAWAY: u8 = 7;
const END_STREAM: u8 = 1;
const ACK: u8 = 1;
const END_HEADERS: u8 = 4;
const PADDED: u8 = 8;
const SETTINGS_ENABLE_PUSH: u16 = 2;

/// What the gateway answered: its status and the typed payload, if any.
pub type Answer = (u32, Option<(String, Vec<u8>)>);

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_bytes(out: &mut Vec<u8>, field: u64, v: &[u8]) {
    put_varint(out, field << 3 | 2);
    put_varint(out, v.len() as u64);
    out.extend_from_slice(v);
}

fn put_uint(out: &mut Vec<u8>, field: u64, v: u64) {
    put_varint(out, field << 3);
    put_varint(out, v);
}

fn read_varint(b: &[u8], p: &mut usize) -> Option<u64> {
    let (mut r, mut s) = (0u64, 0u32);
    loop {
        let x = *b.get(*p)?;
        *p += 1;
        r |= u64::from(x & 0x7f).checked_shl(s)?;
        if x < 0x80 {
            return Some(r);
        }
        s += 7;
        if s > 63 {
            return None;
        }
    }
}

/// A decoded top-level field: a varint, or a length-delimited value.
#[derive(Debug, PartialEq, Eq)]
enum Val {
    Int(u64),
    Bytes(Vec<u8>),
}

fn walk(b: &[u8]) -> Option<Vec<(u64, Val)>> {
    let mut out = Vec::new();
    let mut p = 0;
    while p < b.len() {
        let k = read_varint(b, &mut p)?;
        match k & 7 {
            0 => out.push((k >> 3, Val::Int(read_varint(b, &mut p)?))),
            2 => {
                let l = usize::try_from(read_varint(b, &mut p)?).ok()?;
                let end = p.checked_add(l).filter(|e| *e <= b.len())?;
                out.push((k >> 3, Val::Bytes(b[p..end].to_vec())));
                p = end;
            }
            // Fixed-width values: skipped, bounds-checked.
            1 | 5 => {
                let w = if k & 7 == 1 { 8 } else { 4 };
                p = p.checked_add(w).filter(|e| *e <= b.len())?;
            }
            _ => return None,
        }
    }
    Some(out)
}

fn bytes_of(f: &[(u64, Val)], n: u64) -> Option<&[u8]> {
    f.iter().find_map(|(k, v)| match v {
        Val::Bytes(b) if *k == n => Some(b.as_slice()),
        _ => None,
    })
}

/// This process's `application_instance_uuid`: the real client sends one per
/// process on every request, and the server pins a pilot to the first it sees.
fn instance_uuid() -> &'static [u8; 16] {
    static ID: std::sync::OnceLock<[u8; 16]> = std::sync::OnceLock::new();
    ID.get_or_init(|| {
        // The std hasher's keys are random per process: no clock needed.
        use std::hash::{BuildHasher, Hasher};
        let mut id = [0u8; 16];
        for (i, half) in id.chunks_mut(8).enumerate() {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u32(std::process::id() ^ i as u32);
            half.copy_from_slice(&h.finish().to_le_bytes());
        }
        id
    })
}

/// One `eve_public.Request` for `character` (0 leaves the context out).
pub fn encode_request(character: i64, ty: &str, body: &[u8], correlation: &[u8]) -> Vec<u8> {
    let mut any = Vec::new();
    put_bytes(&mut any, 1, format!("type.googleapis.com/{ty}").as_bytes());
    if !body.is_empty() {
        put_bytes(&mut any, 2, body);
    }
    let mut r = Vec::new();
    put_bytes(&mut r, 2, correlation);
    put_bytes(&mut r, 3, &any);
    put_bytes(&mut r, 5, instance_uuid());
    if character != 0 {
        let mut id = Vec::new();
        put_uint(&mut id, 1, character as u64);
        let mut who = Vec::new();
        put_bytes(&mut who, 1, &id);
        let mut ctx = Vec::new();
        put_bytes(&mut ctx, 10, &who);
        put_bytes(&mut r, 6, &ctx);
    }
    r
}

/// `(correlation, (status, payload))` of one `eve_public.Response`. The payload
/// is `(type name after the last '/', value)`.
pub fn decode_response(resp: &[u8]) -> Result<(Vec<u8>, Answer)> {
    let bad = || Error::Protocol("gateway response is not a protobuf message".into());
    let f = walk(resp).ok_or_else(bad)?;
    let status = f
        .iter()
        .find_map(|(k, v)| match v {
            Val::Int(n) if *k == 3 => Some(*n),
            _ => None,
        })
        .unwrap_or(0);
    let corr = bytes_of(&f, 2).unwrap_or(&[]).to_vec();
    let payload = match bytes_of(&f, 5) {
        None => None,
        Some(any) => {
            let a = walk(any).ok_or_else(bad)?;
            let url = String::from_utf8_lossy(bytes_of(&a, 1).unwrap_or(&[])).into_owned();
            let name = url.rsplit('/').next().unwrap_or("").to_owned();
            Some((name, bytes_of(&a, 2).unwrap_or(&[]).to_vec()))
        }
    };
    Ok((corr, (status as u32, payload)))
}

/// The 5-byte gRPC prefix (uncompressed, big-endian length) around a message.
pub fn grpc_message(m: &[u8]) -> Result<Vec<u8>> {
    let n = u32::try_from(m.len())
        .map_err(|_| Error::Protocol("gateway request is longer than a gRPC message".into()))?;
    let mut f = vec![0];
    f.extend_from_slice(&n.to_be_bytes());
    f.extend_from_slice(m);
    Ok(f)
}

/// The message inside a gRPC data stream, or why it is not one.
pub fn grpc_unwrap(b: &[u8]) -> Result<&[u8]> {
    if b.len() < 5 || b[0] != 0 {
        return Err(Error::Protocol("gateway data lacks the gRPC prefix".into()));
    }
    let n = u32::from_be_bytes([b[1], b[2], b[3], b[4]]) as usize;
    5usize
        .checked_add(n)
        .and_then(|end| b.get(5..end))
        .ok_or_else(|| Error::Protocol("gateway data is shorter than its gRPC length".into()))
}

/// One HTTP/2 frame.
pub fn frame(kind: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let n = payload.len();
    debug_assert!(n < 1 << 24);
    let mut f = vec![(n >> 16) as u8, (n >> 8) as u8, n as u8, kind, flags];
    f.extend_from_slice(&(stream & 0x7fff_ffff).to_be_bytes());
    f.extend_from_slice(payload);
    f
}

/// A literal header, no indexing, new name, nothing Huffman coded. Names and
/// values stay under 127 bytes here, so one length byte is enough.
fn literal(out: &mut Vec<u8>, name: &str, value: &str) {
    debug_assert!(name.len() < 127 && value.len() < 127);
    out.push(0);
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
    out.push(value.len() as u8);
    out.extend_from_slice(value.as_bytes());
}

/// The HEADERS block of one `Send` request.
pub fn request_headers(authority: &str) -> Vec<u8> {
    let mut h = Vec::new();
    literal(&mut h, ":method", "POST");
    literal(&mut h, ":scheme", "http");
    literal(&mut h, ":path", PATH);
    literal(&mut h, ":authority", authority);
    literal(&mut h, "content-type", "application/grpc");
    literal(&mut h, "te", "trailers");
    h
}

/// The DATA frames of one request body: at most `CHUNK` bytes each, END_STREAM
/// on the last only.
pub fn data_frames(stream: u32, msg: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chunks = msg.chunks(CHUNK).peekable();
    if chunks.peek().is_none() {
        return frame(DATA, END_STREAM, stream, &[]);
    }
    while let Some(c) = chunks.next() {
        let flags = if chunks.peek().is_none() {
            END_STREAM
        } else {
            0
        };
        out.extend(frame(DATA, flags, stream, c));
    }
    out
}

/// The WINDOW_UPDATEs that keep both flow-control windows open after a DATA
/// frame of `len` bytes on our stream (the stream's only while it stays open).
pub fn window_updates(stream: u32, len: usize, ended: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if len > 0 {
        let inc = (len as u32).to_be_bytes();
        out.extend(frame(WINDOW_UPDATE, 0, 0, &inc));
        if !ended {
            out.extend(frame(WINDOW_UPDATE, 0, stream, &inc));
        }
    }
    out
}

/// The data of a DATA frame without its padding.
fn unpad(flags: u8, body: &[u8]) -> Result<&[u8]> {
    if flags & PADDED == 0 {
        return Ok(body);
    }
    let bad = || Error::Protocol("padded gateway DATA is malformed".into());
    let (&pad, rest) = body.split_first().ok_or_else(bad)?;
    let keep = rest.len().checked_sub(usize::from(pad)).ok_or_else(bad)?;
    Ok(&rest[..keep])
}

/// Unwrap the gRPC data of one stream, decode the `Response` and check that it
/// answers `corr`.
pub fn check_answer(data: &[u8], corr: &[u8]) -> Result<Answer> {
    if data.is_empty() {
        return Err(Error::Protocol(
            "gateway stream ended with no response".into(),
        ));
    }
    let (got, answer) = decode_response(grpc_unwrap(data)?)?;
    if got != corr {
        return Err(Error::Protocol(
            "gateway answered another request's correlation".into(),
        ));
    }
    Ok(answer)
}

/// A connection to the gateway. One request at a time. After any error from
/// `send` the connection is `broken`: the stream may stop mid-frame, so every
/// later `send` fails at once and the caller reconnects with `connect`.
pub struct GatewayClient {
    s: TcpStream,
    authority: String,
    timeout: Duration,
    next_stream: u32,
    next_corr: u64,
    broken: bool,
}

impl GatewayClient {
    /// Connect and open the HTTP/2 connection: preface and an empty SETTINGS.
    pub fn connect(addr: SocketAddr, timeout: Duration) -> Result<Self> {
        let mut s = TcpStream::connect_timeout(&addr, timeout)?;
        s.set_read_timeout(Some(timeout))?;
        s.set_write_timeout(Some(timeout))?;
        let mut out = PREFACE.to_vec();
        // SETTINGS_ENABLE_PUSH = 0: we never accept a pushed stream.
        let mut push = SETTINGS_ENABLE_PUSH.to_be_bytes().to_vec();
        push.extend_from_slice(&0u32.to_be_bytes());
        out.extend(frame(SETTINGS, 0, 0, &push));
        s.write_all(&out)?;
        Ok(GatewayClient {
            s,
            authority: addr.to_string(),
            timeout,
            next_stream: 1,
            next_corr: 1,
            broken: false,
        })
    }

    fn read_frame(&mut self) -> Result<(u8, u8, u32, Vec<u8>)> {
        let mut h = [0u8; 9];
        self.s.read_exact(&mut h)?;
        let len = usize::from(h[0]) << 16 | usize::from(h[1]) << 8 | usize::from(h[2]);
        if len > MAX_FRAME {
            return Err(Error::Protocol(format!("gateway frame of {len} bytes")));
        }
        let sid = u32::from_be_bytes([h[5], h[6], h[7], h[8]]) & 0x7fff_ffff;
        let mut body = vec![0u8; len];
        self.s.read_exact(&mut body)?;
        Ok((h[3], h[4], sid, body))
    }

    /// Send one request for `character` (0: no pilot context) and read its
    /// stream to the end. Returns the status and the typed payload. On any
    /// error the connection is unusable: reconnect.
    pub fn send(&mut self, character: i64, ty: &str, body: &[u8]) -> Result<Answer> {
        if self.broken {
            return Err(Error::Protocol(
                "gateway connection is closed; reconnect".into(),
            ));
        }
        let r = self.exchange(character, ty, body);
        if r.is_err() {
            self.broken = true;
        }
        r
    }

    fn exchange(&mut self, character: i64, ty: &str, body: &[u8]) -> Result<Answer> {
        let deadline = clock::wall() + self.timeout;
        let stream = self.next_stream;
        self.next_stream = stream
            .checked_add(2)
            .filter(|n| *n <= 0x7fff_ffff)
            .ok_or_else(|| Error::Protocol("gateway stream ids are used up; reconnect".into()))?;
        let corr = self.next_corr.to_be_bytes().to_vec();
        self.next_corr += 1;
        let msg = grpc_message(&encode_request(character, ty, body, &corr))?;
        let mut out = frame(
            HEADERS,
            END_HEADERS,
            stream,
            &request_headers(&self.authority),
        );
        out.extend(data_frames(stream, &msg));
        self.s.write_all(&out)?;
        let mut data = Vec::new();
        loop {
            if clock::wall() > deadline {
                return Err(Error::Protocol("gateway request timed out".into()));
            }
            let (kind, flags, sid, body) = self.read_frame()?;
            match kind {
                SETTINGS if flags & ACK == 0 => {
                    self.s.write_all(&frame(SETTINGS, ACK, 0, &[]))?;
                }
                PING if flags & ACK == 0 => self.s.write_all(&frame(PING, ACK, 0, &body))?,
                GOAWAY => return Err(Error::Protocol("gateway sent GOAWAY".into())),
                RST_STREAM if sid == stream => {
                    return Err(Error::Protocol("gateway reset the stream".into()));
                }
                DATA if sid == stream => {
                    let ended = flags & END_STREAM != 0;
                    let b = unpad(flags, &body)?;
                    if data.len() + b.len() > MAX_RESPONSE {
                        return Err(Error::Protocol("gateway response is too large".into()));
                    }
                    data.extend_from_slice(b);
                    let w = window_updates(stream, body.len(), ended);
                    if !w.is_empty() {
                        self.s.write_all(&w)?;
                    }
                    if ended {
                        break;
                    }
                }
                HEADERS if sid == stream && flags & END_STREAM != 0 => break,
                _ => {}
            }
        }
        check_answer(&data, &corr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eve_server::gateway::{Ground, spawn};

    #[test]
    fn request_layout_is_as_written() {
        let r = encode_request(
            9000001,
            "eve_public.invented.thing.api.GetRequest",
            b"xy",
            b"c1",
        );
        let f = walk(&r).unwrap();
        assert_eq!(bytes_of(&f, 2), Some(&b"c1"[..]));
        let any = walk(bytes_of(&f, 3).unwrap()).unwrap();
        assert_eq!(
            bytes_of(&any, 1),
            Some(&b"type.googleapis.com/eve_public.invented.thing.api.GetRequest"[..])
        );
        assert_eq!(bytes_of(&any, 2), Some(&b"xy"[..]));
        // 6 -> 10 -> 1 -> 1 holds the pilot.
        let ctx = walk(bytes_of(&f, 6).unwrap()).unwrap();
        let who = walk(bytes_of(&ctx, 10).unwrap()).unwrap();
        let id = walk(bytes_of(&who, 1).unwrap()).unwrap();
        assert_eq!(id, vec![(1, Val::Int(9000001))]);
        // Every request names this process (field 5), the same one each time.
        assert_eq!(bytes_of(&f, 5), Some(&instance_uuid()[..]));
        // No pilot, no context; no body, no value.
        let r = encode_request(0, "a.B", b"", b"c");
        let f = walk(&r).unwrap();
        assert!(bytes_of(&f, 6).is_none());
        assert!(bytes_of(&walk(bytes_of(&f, 3).unwrap()).unwrap(), 2).is_none());
    }

    #[test]
    fn response_decodes_status_correlation_and_type() {
        let mut any = Vec::new();
        put_bytes(&mut any, 1, b"type.googleapis.com/invented.pkg.Thing");
        put_bytes(&mut any, 2, b"\x01\x02");
        let mut r = Vec::new();
        put_bytes(&mut r, 2, b"k1");
        put_uint(&mut r, 3, 200);
        put_bytes(&mut r, 5, &any);
        assert_eq!(
            decode_response(&r).unwrap(),
            (
                b"k1".to_vec(),
                (200, Some(("invented.pkg.Thing".to_owned(), vec![1, 2])))
            )
        );
        let mut r = Vec::new();
        put_uint(&mut r, 3, 404);
        assert_eq!(decode_response(&r).unwrap(), (vec![], (404, None)));
        assert!(decode_response(&[0x12, 0x09, 1]).is_err());
    }

    #[test]
    fn grpc_prefix_and_frames_are_exact() {
        let m = grpc_message(b"abc").unwrap();
        assert_eq!(m, vec![0, 0, 0, 0, 3, b'a', b'b', b'c']);
        assert_eq!(grpc_unwrap(&m).unwrap(), b"abc");
        assert!(grpc_unwrap(&m[..7]).is_err());
        assert!(grpc_unwrap(&[1, 0, 0, 0, 0]).is_err());
        assert!(grpc_unwrap(&[0, 0xff, 0xff, 0xff, 0xff, 1]).is_err());
        let f = frame(HEADERS, END_HEADERS, 3, b"zz");
        assert_eq!(f, vec![0, 0, 2, 1, 4, 0, 0, 0, 3, b'z', b'z']);
        let h = request_headers("h:1");
        assert!(h.starts_with(&[0, 7, b':', b'm']));
        assert!(h.windows(PATH.len()).any(|w| w == PATH.as_bytes()));
    }

    fn client() -> GatewayClient {
        let addr = spawn("127.0.0.1:0".parse().unwrap(), Ground::local()).unwrap();
        GatewayClient::connect(addr, Duration::from_secs(5)).unwrap()
    }

    #[test]
    fn asks_the_servers_own_gateway_in_process() {
        let mut c = client();
        let (status, payload) = c
            .send(
                9000001,
                "eve_public.sovereignty.mercenaryden.activity.api.GetAllRequest",
                b"",
            )
            .unwrap();
        assert_eq!(status, 200);
        assert_eq!(
            payload.unwrap().0,
            "eve_public.sovereignty.mercenaryden.activity.api.GetAllResponse"
        );
        // A second request on the same connection, an unknown den.
        let (status, payload) = c
            .send(
                9000001,
                "eve_public.sovereignty.mercenaryden.api.GetAsOwnerRequest",
                b"",
            )
            .unwrap();
        assert_eq!(status, 404);
        assert!(payload.is_none());
    }

    use std::net::TcpListener;

    fn plan_body(name: &[u8]) -> Vec<u8> {
        let mut attrs = Vec::new();
        put_bytes(&mut attrs, 1, name);
        let mut create = Vec::new();
        put_bytes(&mut create, 1, &attrs);
        create
    }

    const PLAN: &str = "eve_public.character.skill.plan";

    #[test]
    fn the_pilot_context_reaches_the_server() {
        let mut c = client();
        let (status, _) = c
            .send(
                9000001,
                &format!("{PLAN}.CreateRequest"),
                &plan_body(b"invented plan"),
            )
            .unwrap();
        assert_eq!(status, 200);
        let list = |c: &mut GatewayClient, who: i64| {
            let (status, p) = c.send(who, &format!("{PLAN}.GetAllRequest"), b"").unwrap();
            assert_eq!(status, 200);
            let (name, v) = p.unwrap();
            assert!(name.ends_with("GetAllResponse"));
            v
        };
        assert!(!list(&mut c, 9000001).is_empty());
        assert!(list(&mut c, 9000002).is_empty());
    }

    #[test]
    fn a_licence_body_is_read_by_the_server() {
        let mut c = client();
        let ask = |c: &mut GatewayClient, kind: &[u8]| {
            let mut id = Vec::new();
            put_bytes(&mut id, 1, kind);
            let mut req = Vec::new();
            put_bytes(&mut req, 1, &id);
            c.send(9000001, "eve_public.user.license.api.GetRequest", &req)
                .unwrap()
        };
        let (status, payload) = ask(&mut c, b"eve_clonestate_omega");
        assert_eq!(status, 200);
        assert!(!payload.unwrap().1.is_empty());
        let (status, payload) = ask(&mut c, b"invented_license");
        assert_eq!(status, 404);
        assert!(payload.is_none());
    }

    #[test]
    fn check_answer_compares_the_correlation() {
        let mut r = Vec::new();
        put_bytes(&mut r, 2, b"k1");
        put_uint(&mut r, 3, 200);
        let m = grpc_message(&r).unwrap();
        assert_eq!(check_answer(&m, b"k1").unwrap(), (200, None));
        assert!(check_answer(&m, b"k2").is_err());
        let mut r = Vec::new();
        put_uint(&mut r, 3, 200);
        let m = grpc_message(&r).unwrap();
        assert!(check_answer(&m, b"k1").is_err());
        assert!(check_answer(&[], b"k1").is_err());
    }

    #[test]
    fn a_failed_send_breaks_the_connection() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.write_all(&frame(GOAWAY, 0, 0, &[0; 8])).unwrap();
            let mut sink = Vec::new();
            let _ = s.read_to_end(&mut sink);
        });
        let mut c = GatewayClient::connect(addr, Duration::from_secs(5)).unwrap();
        let e = c.send(0, "a.B", b"").unwrap_err().to_string();
        assert!(e.contains("GOAWAY"), "{e}");
        let e = c.send(0, "a.B", b"").unwrap_err().to_string();
        assert!(e.contains("closed; reconnect"), "{e}");
        drop(c);
        t.join().unwrap();
    }

    #[test]
    fn request_data_is_chunked_and_windows_are_kept_open() {
        let msg = vec![7u8; CHUNK * 2 + 10];
        let out = data_frames(5, &msg);
        let mut p = 0;
        let mut seen = Vec::new();
        while p < out.len() {
            let n =
                usize::from(out[p]) << 16 | usize::from(out[p + 1]) << 8 | usize::from(out[p + 2]);
            seen.push((n, out[p + 4]));
            p += 9 + n;
        }
        assert_eq!(seen, vec![(CHUNK, 0), (CHUNK, 0), (10, END_STREAM)]);
        assert_eq!(data_frames(5, &[]).len(), 9);
        // Open stream: both windows; ended stream or empty frame: fewer.
        assert_eq!(window_updates(5, 100, false).len(), 26);
        assert_eq!(window_updates(5, 100, true).len(), 13);
        assert!(window_updates(5, 0, false).is_empty());
    }

    #[test]
    fn padding_and_fixed_width_fields_are_bounds_checked() {
        assert_eq!(unpad(0, b"abc").unwrap(), b"abc");
        assert_eq!(unpad(PADDED, &[2, b'a', 0, 0]).unwrap(), b"a");
        assert!(unpad(PADDED, &[3, b'a', 0]).is_err());
        assert!(unpad(PADDED, &[]).is_err());
        // Field 1 as fixed64, field 2 as fixed32, then a varint field 3.
        let mut b = vec![0x09];
        b.extend_from_slice(&[0; 8]);
        b.push(0x15);
        b.extend_from_slice(&[0; 4]);
        b.extend_from_slice(&[0x18, 5]);
        assert_eq!(walk(&b).unwrap(), vec![(3, Val::Int(5))]);
        assert!(walk(&[0x09, 0, 0]).is_none());
    }

    #[test]
    fn stream_ids_run_out_into_an_error() {
        let mut c = client();
        c.next_stream = 0x7fff_ffff;
        assert!(c.send(0, "a.B", b"").is_err());
        assert!(c.send(0, "a.B", b"").is_err());
    }
}
