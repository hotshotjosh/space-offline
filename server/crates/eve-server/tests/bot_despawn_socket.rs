//! **The log-off path removes a pilot whose connection is closed** (#161), over
//! a real socket. A regression test for the server half of a despawn only: the
//! bot's thread closes its socket with `shutdown(Both)` (`Handle::kick`,
//! eve-bot `driver.rs`), and this test closes a mock pilot's socket the same
//! two ways (shut down, dropped). It does not run `/bots despawn` or the bot
//! driver. A pilot logs in and undocks, the connection closes, the test waits
//! for the log-off hold, runs `crimewatch::tick` to its end itself (not the
//! world clock), and asserts the ship has left the **roster**, which
//! `scan::balls_in_system` reads (`scan` is crate-private, so that function is
//! not called here). **Every id is invented.**

use eve_marshal::PyValue;
use eve_proto::{try_split_frame, write_frame};
use eve_server::server::{Config, Mode, Server};
use eve_server::{dumps, loads};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

const CHALLENGE_LEN: usize = 64;

const ALICE: i64 = 90_161_001;
const ALICE_SHIP: i64 = 100_161_001;

struct MockClient {
    stream: TcpStream,
    buf: Vec<u8>,
}

impl MockClient {
    fn connect(addr: SocketAddr) -> Self {
        let stream = TcpStream::connect(addr).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        MockClient {
            stream,
            buf: Vec::new(),
        }
    }

    fn send(&mut self, v: &PyValue) {
        let mut framed = Vec::new();
        write_frame(&dumps(v), &mut framed).expect("frame");
        self.stream.write_all(&framed).expect("write");
    }

    /// The next frame, skipping the server's own `machoNet` calls.
    fn recv(&mut self) -> PyValue {
        loop {
            let frame = self.recv_raw();
            if let PyValue::Instance { class, .. } = &frame
                && String::from_utf8_lossy(class).rsplit('.').next() == Some("CallReq")
            {
                continue;
            }
            return frame;
        }
    }

    fn recv_raw(&mut self) -> PyValue {
        let mut chunk = [0u8; 4096];
        loop {
            if let Some((frame, consumed)) = try_split_frame(&self.buf).expect("split") {
                let payload = frame.payload.to_vec();
                self.buf.drain(..consumed);
                return loads(&payload).expect("payload parses");
            }
            let n = self.stream.read(&mut chunk).expect("read");
            assert_ne!(n, 0, "server closed while a payload was expected");
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }
}

fn bstr(s: &str) -> PyValue {
    PyValue::Bytes(s.as_bytes().to_vec())
}

fn dict(kv: Vec<(&str, PyValue)>) -> PyValue {
    PyValue::Dict(kv.into_iter().map(|(k, v)| (v, bstr(k))).collect())
}

fn h2() -> PyValue {
    PyValue::Tuple(vec![
        PyValue::Int(i64::from(eve_marshal::build::reference().line.secret)),
        PyValue::Int(i64::from(eve_marshal::build::reference().line.macho)),
        PyValue::Int(0),
        PyValue::Real(eve_marshal::build::reference().line.version),
        PyValue::Int(i64::from(eve_marshal::build::REFERENCE)),
        bstr(eve_marshal::build::reference().line.codename_at_region),
    ])
}

fn request_dict() -> PyValue {
    dict(vec![
        (
            "macho_version",
            PyValue::Int(i64::from(eve_marshal::build::reference().line.macho)),
        ),
        (
            "boot_version",
            PyValue::Real(eve_marshal::build::reference().line.version),
        ),
        (
            "boot_build",
            PyValue::Int(i64::from(eve_marshal::build::REFERENCE)),
        ),
        (
            "boot_codename",
            bstr(eve_marshal::build::reference().line.codename),
        ),
        ("boot_region", bstr("ccp")),
        ("user_name", bstr("EVE-SSO-CONNECTION")),
        ("user_password", PyValue::None),
        ("user_password_hash", PyValue::Bytes(vec![0u8; 20])),
        ("user_languageid", bstr("EN")),
        ("user_affiliateid", PyValue::Int(0)),
        ("user_sso_token", bstr("token")),
    ])
}

fn crypto_hash(args: &[PyValue]) -> PyValue {
    PyValue::Bytes(eve_server::crypto_hash(args).into_bytes())
}

/// The client side of H1..H10.
fn full_handshake(client: &mut MockClient) {
    let PyValue::Tuple(h1) = client.recv() else {
        panic!("H1 is not a tuple")
    };
    assert_eq!(h1.len(), 7);
    client.send(&h2());
    client.send(&PyValue::Tuple(vec![
        PyValue::None,
        bstr("VK"),
        bstr("name-hash"),
    ]));
    client.send(&PyValue::Tuple(vec![
        bstr("placebo"),
        PyValue::Dict(vec![]),
    ]));
    assert_eq!(client.recv(), bstr("OK CC"));
    let client_challenge = vec![0x5Au8; CHALLENGE_LEN];
    client.send(&PyValue::Tuple(vec![
        PyValue::Bytes(client_challenge),
        request_dict(),
    ]));
    assert_eq!(client.recv(), PyValue::Int(2));
    let PyValue::Tuple(h8) = client.recv() else {
        panic!("H8 is not a tuple")
    };
    let PyValue::Bytes(server_challenge) = &h8[0] else {
        panic!("H8[0] is the server challenge")
    };
    client.send(&PyValue::Tuple(vec![
        crypto_hash(&[PyValue::Bytes(server_challenge.clone())]),
        bstr(""),
        PyValue::Dict(vec![]),
    ]));
    let _h10 = client.recv();
}

/// The `SessionInitialStateNotification` after H10.
fn read_session(client: &mut MockClient) {
    let PyValue::Instance { class, .. } = client.recv() else {
        panic!("the session packet is an instance")
    };
    assert_eq!(
        String::from_utf8_lossy(&class),
        "carbon.common.script.net.machoNetPacket.SessionInitialStateNotification"
    );
}

fn call_req_args(service: &str, method: &str, call_id: i64, args: Vec<PyValue>) -> PyValue {
    let payload = eve_proto::CallPayload::Service {
        method: method.to_owned(),
        args,
        kwargs: vec![],
    };
    eve_proto::MachoPacket::new(
        eve_marshal::MachoNetMsg::CallReq,
        eve_proto::MachoAddress::Client {
            client_id: 1,
            call_id: Some(call_id),
            service: None,
        },
        eve_proto::MachoAddress::Any {
            service: Some(service.to_owned()),
            call_id: Some(call_id),
        },
        vec![payload.to_payload()],
    )
    .to_value()
    .expect("a CallReq value")
}

fn expect_call_rsp(v: &PyValue) -> PyValue {
    let PyValue::Instance { class, state } = v else {
        panic!("not an instance: {v:?}")
    };
    assert_eq!(
        String::from_utf8_lossy(class),
        "carbon.common.script.net.machoNetPacket.CallRsp"
    );
    let PyValue::Tuple(t) = &**state else {
        panic!("state is not a tuple")
    };
    let PyValue::Tuple(body) = &t[4] else {
        panic!("body is not a tuple")
    };
    let PyValue::Bytes(pickle) = &body[0] else {
        panic!("a CallRsp payload is a marshalled string")
    };
    loads(pickle).expect("the response pickle loads")
}

/// `(event, args)` of a `Notification`.
fn expect_notification(v: &PyValue) -> (String, Vec<PyValue>) {
    let PyValue::Instance { class, state } = v else {
        panic!("not an instance: {v:?}")
    };
    assert_eq!(
        String::from_utf8_lossy(class),
        "carbon.common.script.net.machoNetPacket.Notification"
    );
    let PyValue::Tuple(t) = &**state else {
        panic!("state is not a tuple")
    };
    let PyValue::Instance { state: dest, .. } = &t[2] else {
        panic!("destination is not an address instance")
    };
    let PyValue::Tuple(dest) = &**dest else {
        panic!("the address state is not a tuple")
    };
    let PyValue::Bytes(event) = &dest[1] else {
        panic!("broadcastID must be a byte string")
    };
    let PyValue::Tuple(body) = &t[4] else {
        panic!("body is not a tuple")
    };
    let PyValue::Tuple(outer) = &body[0] else {
        panic!("the payload is a tuple")
    };
    let PyValue::Bytes(pickle) = &outer[1] else {
        panic!("the inner payload must be a byte string")
    };
    let PyValue::Tuple(after_object) = loads(pickle).expect("the inner pickle") else {
        panic!("the pickle is a tuple")
    };
    let PyValue::Tuple(after_service) = &after_object[1] else {
        panic!("payload[1] is a tuple")
    };
    let PyValue::Tuple(args) = &after_service[1] else {
        panic!("args is a tuple")
    };
    (String::from_utf8_lossy(event).into_owned(), args.clone())
}

fn expect_session_change(v: &PyValue) -> PyValue {
    let PyValue::Instance { class, state } = v else {
        panic!("not an instance: {v:?}")
    };
    assert_eq!(
        String::from_utf8_lossy(class),
        "carbon.common.script.net.machoNetPacket.SessionChangeNotification"
    );
    let PyValue::Tuple(t) = &**state else {
        panic!("state is not a tuple")
    };
    let PyValue::Tuple(body) = &t[4] else {
        panic!("body is not a tuple")
    };
    let PyValue::Tuple(wrapped) = &body[1] else {
        panic!("the change is (0, change)")
    };
    wrapped[1].clone()
}

fn select_character(client: &mut MockClient, character_id: i64, call_id: i64) {
    client.send(&call_req_args(
        "charUnboundMgr",
        "SelectCharacterID",
        call_id,
        vec![
            PyValue::Int(character_id),
            PyValue::None,
            PyValue::Bool(false),
        ],
    ));
    let _ = expect_session_change(&client.recv());
    let _ = expect_call_rsp(&client.recv());
}

/// Read until a `CallRsp`, keeping every notification skipped on the way.
fn call_rsp_collecting(client: &mut MockClient) -> (PyValue, Vec<(String, Vec<PyValue>)>) {
    let mut frames = Vec::new();
    for _ in 0..16 {
        let frame = client.recv();
        let PyValue::Instance { class, .. } = &frame else {
            panic!("expected a packet, got {frame:?}")
        };
        match String::from_utf8_lossy(class).rsplit('.').next() {
            Some("Notification") => frames.push(expect_notification(&frame)),
            Some("SessionChangeNotification") => continue,
            _ => return (expect_call_rsp(&frame), frames),
        }
    }
    panic!("no CallRsp after 16 frames; skipped {frames:?}")
}

/// Undock, as the client does after the session change: the `ship` bind with
/// `Undock` fused on, then the ballpark's bind, which seats the pilot in
/// space.
fn undock(client: &mut MockClient, first_call_id: i64) {
    client.send(&call_req_args(
        "ship",
        "MachoBindObject",
        first_call_id,
        vec![
            PyValue::Tuple(vec![PyValue::Int(60_012_526), PyValue::Int(5)]),
            PyValue::Tuple(vec![
                bstr("Undock"),
                PyValue::Tuple(vec![]),
                PyValue::Dict(vec![]),
            ]),
        ],
    ));
    let _ = call_rsp_collecting(client);
    client.send(&call_req_args(
        "beyonce",
        "MachoResolveObject",
        first_call_id + 1,
        vec![PyValue::Int(30_000_001)],
    ));
    let _ = call_rsp_collecting(client);
    client.send(&call_req_args(
        "beyonce",
        "MachoBindObject",
        first_call_id + 2,
        vec![PyValue::Int(30_000_001), PyValue::None],
    ));
    let _ = call_rsp_collecting(client);
}

/// Log in, undock, and close the connection by `close`; then run the clock to
/// the end of the hold.
fn despawned_pilot_leaves_space(close: impl FnOnce(MockClient)) {
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.mode = Mode::Gps;
    config.sandbox.characters = vec![eve_server::model::Character {
        character_id: ALICE,
        name: "Alice".to_owned(),
        ship_id: ALICE_SHIP,
        ..eve_server::model::Character::default()
    }];
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);
    select_character(&mut client, ALICE, 70);
    undock(&mut client, 100);
    assert!(
        world
            .read()
            .expect("the world")
            .roster()
            .get(ALICE)
            .is_some(),
        "undocked: in space"
    );
    close(client);
    // The server notices on its own thread.
    let began = Instant::now();
    let until = loop {
        let held = world
            .read()
            .expect("the world")
            .combat()
            .crimewatch()
            .held_until(ALICE);
        if let Some(until) = held {
            break until;
        }
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "the closed connection never put the ship under the log-off hold; online={}",
            world.read().expect("the world").sessions().is_online(ALICE)
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut w = world.write().expect("the world");
    assert!(!w.sessions().is_online(ALICE), "the session is cleared");
    assert!(w.roster().get(ALICE).is_some(), "held while the timer runs");
    eve_server::crimewatch::tick(&mut w, until);
    assert!(
        w.roster().get(ALICE).is_none(),
        "the ship is gone when the hold ends"
    );
}

#[test]
fn a_pilot_whose_socket_is_shut_down_leaves_space_after_the_hold() {
    // `Handle::kick`: `TcpStream::shutdown(Both)`.
    despawned_pilot_leaves_space(|client| {
        let _ = client.stream.shutdown(Shutdown::Both);
        // Kept open until the server has seen the end, like a thread that has
        // not yet dropped its `Conn`.
        std::thread::sleep(Duration::from_millis(200));
        drop(client);
    });
}

#[test]
fn a_pilot_whose_socket_is_dropped_leaves_space_after_the_hold() {
    despawned_pilot_leaves_space(drop);
}
