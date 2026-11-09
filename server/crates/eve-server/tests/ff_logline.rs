//! `logline::init_to` writes the process log to a file (#147). Its own test
//! binary because the logger is a once-only process global.

use std::time::Duration;

#[test]
fn init_to_writes_the_file() {
    let dir = std::env::temp_dir().join(format!("ff-logline-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("host.log");
    eve_server::logline::init_to(&path, false).unwrap();
    eve_server::outln!("first {}", 1);
    eve_server::errln!("second");
    eve_server::logline::shutdown(Duration::from_secs(5));
    // a second init (to anywhere) is a no-op
    eve_server::logline::init_to(&dir.join("other.log"), false).unwrap();
    assert!(!dir.join("other.log").exists());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "first 1\nsecond\n");
    let _ = std::fs::remove_dir_all(&dir);
}
