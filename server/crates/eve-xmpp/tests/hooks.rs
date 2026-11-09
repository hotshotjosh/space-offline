//! The `Hooks` seam (#150). `set_hooks` is process-global, so this is its own
//! test binary and one test function: nothing else here may run first.
//!
//! The scripted server speaks just enough XMPP for `Client::login`, then goes
//! silent. All ids and numbers are invented.

use eve_xmpp::{Client, Hooks, set_hooks};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static SENT: AtomicU64 = AtomicU64::new(0);
static READ: AtomicU64 = AtomicU64::new(0);
static PEERS_OK: AtomicU64 = AtomicU64::new(0);

const HEADER: &str = "<?xml version='1.0'?><stream:stream from='chat.test' id='s1' version='1.0' xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams'>";

fn tenth(d: Duration) -> Duration {
    d / 10
}

fn sent(peer: SocketAddr, n: u64) {
    if peer.port() != 0 {
        PEERS_OK.fetch_add(1, Ordering::Relaxed);
    }
    SENT.fetch_add(n, Ordering::Relaxed);
}

fn read(_peer: SocketAddr, n: u64) {
    READ.fetch_add(n, Ordering::Relaxed);
}

/// Read until `needle` has been seen `times` times in all; returns what it read.
fn wait_for(s: &mut std::net::TcpStream, acc: &mut String, needle: &str, times: usize) {
    let mut buf = [0u8; 4096];
    while acc.matches(needle).count() < times {
        let n = s.read(&mut buf).expect("server read");
        assert!(n > 0, "client hung up early");
        acc.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
}

/// Returns (bytes received, bytes written) when the login is done, then holds
/// the socket open and silent until the client is dropped.
fn script(l: TcpListener) -> thread::JoinHandle<(u64, u64)> {
    thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        let mut acc = String::new();
        let mut wrote = 0u64;
        let mut put = |s: &mut std::net::TcpStream, t: &str| {
            s.write_all(t.as_bytes()).unwrap();
            wrote += t.len() as u64;
        };
        wait_for(&mut s, &mut acc, "<stream:stream", 1);
        put(&mut s, &format!("{HEADER}<stream:features/>"));
        wait_for(&mut s, &mut acc, "</auth>", 1);
        put(
            &mut s,
            "<success xmlns='urn:ietf:params:xml:ns:xmpp-sasl'/>",
        );
        wait_for(&mut s, &mut acc, "<stream:stream", 2);
        put(&mut s, &format!("{HEADER}<stream:features/>"));
        wait_for(&mut s, &mut acc, "<bind", 1);
        put(&mut s, "<iq id='id1' type='result'/>");
        wait_for(&mut s, &mut acc, "<session", 1);
        put(&mut s, "<iq id='id2' type='result'/>");
        wait_for(&mut s, &mut acc, "<presence", 1);
        put(&mut s, "<presence from='7@chat.test/eveclient'/>");
        let mut got = acc.len() as u64;
        // silent until the client goes away; a read split across the
        // presence frame still counts the rest
        let mut buf = [0u8; 64];
        while let Ok(n) = s.read(&mut buf) {
            if n == 0 {
                break;
            }
            got += n as u64;
        }
        (got, wrote)
    })
}

#[test]
fn installed_hooks_scale_the_read_timeout_and_count_the_bytes() {
    set_hooks(Hooks {
        timeout: tenth,
        sent,
        read,
    })
    .unwrap_or_else(|_| panic!("first set_hooks is accepted"));
    // set once: a second install is refused
    assert!(set_hooks(Hooks::IDENTITY).is_err());

    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let server = script(l);
    let mut c = Client::login(addr, "chat.test", 7, "tok").expect("login under hooks");

    // A line that never speaks: the one-second wait is a tenth of that.
    let t = Instant::now();
    let got = c.recv_timeout(Duration::from_secs(1)).unwrap();
    let took = t.elapsed();
    assert!(got.is_none());
    assert!(took < Duration::from_millis(500), "took {took:?}");
    assert!(took >= Duration::from_millis(50), "took {took:?}");

    drop(c);
    let (received, wrote) = server.join().unwrap();
    assert!(PEERS_OK.load(Ordering::Relaxed) > 0, "peer address given");
    assert_eq!(SENT.load(Ordering::Relaxed), received);
    assert_eq!(READ.load(Ordering::Relaxed), wrote);
}
