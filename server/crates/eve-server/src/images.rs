//! **A local image server for character portraits** (#205), opt-in with
//! `--image-server ADDR`.
//!
//! The client draws every 2D portrait (character select, NEOCOM, chat,
//! show-info) through `EvePhoto`'s `RemoteImageCacher`, which fetches
//! `<imageserverurl>Character/<charID>_<size>.jpg` and caches it
//! **[V decompiled `evePhotosvc.pyc`, `InitializePortraits`]**. The URL is
//! `machoNet.GetGlobalConfig().get('imageserverurl')`, else
//! `defaultImageServerForUser`, which is `None` in a packaged client
//! (`blue.pyos.packaged`). With no URL the cacher never initializes, so
//! every portrait is the default head, even one the character creator
//! saved to the client's cache.
//!
//! Two halves:
//!
//! - **The URL.** `machoNet.OnGlobalConfigUpdated(key, value)` is in
//!   machoNet's `__notifyevents__` and sets one global config key
//!   **[V decompiled `machoNet.pyc`]**. `dispatch` pushes it as a
//!   notification right after the session's initial state, so it lands
//!   before the select screen's first portrait (`InitializePortraits` reads
//!   the URL once). Only with `--image-server`: without it the login frames
//!   are unchanged.
//! - **The pictures.** `GET /Character/<id>_<size>.jpg` serves
//!   `<dir>/<id>_<size>.jpg`, else the largest `<dir>/<id>_<n>.jpg` there
//!   is. `dir` is `--portraits DIR`, default `portraits/` beside the world
//!   file. **[A]** this server renders nothing, so a picture must already
//!   exist: pointing `--portraits` at the client's own
//!   `cache/Pictures/Characters` serves the 512 px snapshot the character
//!   creator saves there. Anything else is 404, and the client keeps its
//!   default.
//!
//! std sockets and threads only, as `admin_web`. GET only, a fixed path
//! shape (no traversal), a capped request head, 5 s timeouts.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The world slot: the URL `dispatch` advertises at login, if any.
#[derive(Debug, Default)]
pub struct Config {
    pub url: Option<String>,
}

/// The registration row; the coordinator adds it to `world_ext::SLOTS`.
pub const SLOT: crate::world_ext::SlotSpec = crate::world_ext::SlotSpec {
    name: "images",
    make: |_| Box::new(Config::default()),
};

/// The global config key the URL goes in **[V]**.
pub const URL_KEY: &str = "imageserverurl";

const MAX_HEAD: usize = 4096;

/// `(charID, size)` of a `/Character/<id>_<size>.jpg` path, else `None`.
pub fn parse_path(path: &str) -> Option<(i64, u32)> {
    let name = path.strip_prefix("/Character/")?.strip_suffix(".jpg")?;
    let (id, size) = name.split_once('_')?;
    if id.is_empty()
        || size.is_empty()
        || !id.bytes().chain(size.bytes()).all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some((id.parse().ok()?, size.parse().ok()?))
}

/// The file to serve for `(id, size)` in `dir`: the exact size, else the
/// largest size on disk.
pub fn pick(dir: &Path, id: i64, size: u32) -> Option<PathBuf> {
    let exact = dir.join(format!("{id}_{size}.jpg"));
    if exact.is_file() {
        return Some(exact);
    }
    let prefix = format!("{id}_");
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let n: u32 = name
                .strip_prefix(&prefix)?
                .strip_suffix(".jpg")?
                .parse()
                .ok()?;
            Some((n, e.path()))
        })
        .max_by_key(|(n, _)| *n)
        .map(|(_, p)| p)
}

fn respond(s: &mut TcpStream, status: &str, kind: &str, body: &[u8]) -> io::Result<()> {
    write!(
        s,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    s.write_all(body)
}

fn serve(mut s: TcpStream, dir: &Path) -> io::Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut r = BufReader::new(s.try_clone()?);
    let mut line = String::new();
    r.read_line(&mut line)?;
    let mut head = line.len();
    // Drain the headers; nothing in them changes the answer.
    loop {
        let mut h = String::new();
        let n = r.read_line(&mut h)?;
        head += n;
        if n == 0 || h == "\r\n" || h == "\n" || head > MAX_HEAD {
            break;
        }
    }
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if method != "GET" {
        return respond(
            &mut s,
            "405 Method Not Allowed",
            "text/plain",
            b"GET only\n",
        );
    }
    let path = path.split('?').next().unwrap_or("");
    match parse_path(path).and_then(|(id, size)| pick(dir, id, size)) {
        Some(file) => {
            let body = std::fs::read(&file)?;
            respond(&mut s, "200 OK", "image/jpeg", &body)
        }
        None => respond(&mut s, "404 Not Found", "text/plain", b"no such portrait\n"),
    }
}

/// Bind `addr` and serve `dir` on a thread per connection.
pub fn open(addr: SocketAddr, dir: PathBuf) -> io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    std::thread::Builder::new()
        .name("images".to_owned())
        .spawn(move || {
            for conn in listener.incoming().flatten() {
                let dir = dir.clone();
                let _ = std::thread::Builder::new()
                    .name("images-conn".to_owned())
                    .spawn(move || {
                        if let Err(e) = serve(conn, &dir) {
                            crate::outln!("[images] request failed: {e}");
                        }
                    });
            }
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_portrait_path_shape_is_accepted() {
        assert_eq!(
            parse_path("/Character/90000005_128.jpg"),
            Some((90_000_005, 128))
        );
        assert_eq!(parse_path("/Character/../x_1.jpg"), None);
        assert_eq!(parse_path("/Character/9_.jpg"), None);
        assert_eq!(parse_path("/Corporation/1_64.png"), None);
        assert_eq!(parse_path("/Character/1_64.jpg/../../etc"), None);
    }

    #[test]
    fn the_exact_size_wins_else_the_largest() {
        let dir = std::env::temp_dir().join(format!("eve-images-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        for n in ["7_64.jpg", "7_512.jpg", "8_32.jpg"] {
            std::fs::write(dir.join(n), b"x").expect("write");
        }
        assert_eq!(pick(&dir, 7, 64), Some(dir.join("7_64.jpg")));
        assert_eq!(pick(&dir, 7, 128), Some(dir.join("7_512.jpg")));
        assert_eq!(pick(&dir, 9, 128), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_request_gets_the_file_or_404() {
        let dir = std::env::temp_dir().join(format!("eve-images-srv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("5_512.jpg"), b"JPEGBYTES").expect("write");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        drop(listener);
        open(addr, dir.clone()).expect("open");
        let get = |path: &str| {
            let mut s = TcpStream::connect(addr).expect("connect");
            write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").expect("send");
            let mut out = Vec::new();
            std::io::Read::read_to_end(&mut s, &mut out).expect("read");
            String::from_utf8_lossy(&out).into_owned()
        };
        let ok = get("/Character/5_128.jpg");
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        assert!(ok.ends_with("JPEGBYTES"));
        assert!(get("/Character/6_128.jpg").starts_with("HTTP/1.1 404"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
