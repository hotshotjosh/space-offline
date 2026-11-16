//! The bot clock lint (#146 FF-0b): no raw time source in production code.
//!
//! Every bot read of time goes through `crate::clock` (the server's one clock
//! seam, `eve_server::game_clock::clock`): `clock::instant()` and
//! `clock::since(x)` for game time, `clock::sleep(d)` for a game wait,
//! `clock::unix_secs()` for the unix second, `clock::system_now()` for a
//! system time, and `clock::wall()` / `clock::wall_since(x)` for real-time
//! measurements (the `[stats]` timing, the control link, samplers, log stamps).
//! A socket wait goes through `Conn::recv_within`.
//!
//! This counts raw time calls per file under `src/`, the way
//! `eve-server/src/clock.rs` `every_time_source_is_sanctioned` does, against
//! [`SANCTIONED`]. The counting machinery below is a copy of that test module's
//! (it is private to eve-server's test build); keep the two in step. The only
//! table entries are sites kept on wall time, each with its reason.

use eve_server::game_clock::clock::USE_SLEEP;

/// The raw time sources the lint counts: eve-server's `clock::PATTERNS`, plus
/// the bots' own unix-second helper and the epoch constant.
const PATTERNS: &[&str] = &[
    "Instant::now",
    "SystemTime::now",
    ".elapsed(",
    "thread::sleep(",
    "set_read_timeout(",
    "set_write_timeout(",
    "park_timeout(",
    "recv_timeout(",
    "wait_timeout",
    "connect_timeout(",
    "Instant as ",
    "SystemTime as ",
    USE_SLEEP,
    "unix_now(",
    "UNIX_EPOCH",
];

/// Every raw time source left in production code, per file and pattern:
/// `(file under src/, pattern, exact count, why)`. A file or pattern not
/// listed must count 0.
const SANCTIONED: &[(&str, &str, usize, &str)] = &[
    (
        "bin/eve-bot.rs",
        "thread::sleep(",
        1,
        "kept real: the one-shot CLI holds its session open for a wall-clock `--hold`",
    ),
    (
        "bin/eve-bots.rs",
        "thread::sleep(",
        1,
        "kept real: the supervisor loop's 500 ms poll of its threads, the stats line, the control directory and the run-length stop",
    ),
    (
        "chat.rs",
        "recv_timeout(",
        1,
        "kept real: a 1 ms poll of the XMPP client's own channel, not a game wait",
    ),
    (
        "control.rs",
        "connect_timeout(",
        1,
        "kept real: the control link is operator plumbing on wall time",
    ),
    (
        "control.rs",
        "set_read_timeout(",
        1,
        "kept real: the control link is operator plumbing on wall time",
    ),
    (
        "control.rs",
        "thread::sleep(",
        1,
        "kept real: the control link's stop-flag poll",
    ),
    (
        "gateway_client.rs",
        "connect_timeout(",
        1,
        "kept real: the gateway HTTP socket, a wall-clock timeout",
    ),
    (
        "gateway_client.rs",
        "set_read_timeout(",
        1,
        "kept real: the gateway HTTP socket, a wall-clock timeout",
    ),
    (
        "gateway_client.rs",
        "set_write_timeout(",
        1,
        "kept real: the gateway HTTP socket, a wall-clock timeout",
    ),
    (
        "lib.rs",
        "set_read_timeout(",
        3,
        "kept real: the socket's own timeout; `connect` and `recv_within` (set and restore) are the only places",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    fn is_ident(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'_'
    }

    /// `src` with comments and the insides of string and char literals
    /// blanked (newlines kept), so braces and patterns are only seen in code.
    fn blank(src: &str) -> Vec<u8> {
        let c = src.as_bytes();
        let n = c.len();
        let mut o = c.to_vec();
        let wipe = |o: &mut Vec<u8>, a: usize, b: usize| {
            for x in &mut o[a..b.min(n)] {
                if *x != b'\n' {
                    *x = b' ';
                }
            }
        };
        let mut i = 0;
        while i < n {
            match c[i] {
                b'/' if c.get(i + 1) == Some(&b'/') => {
                    let s = i;
                    while i < n && c[i] != b'\n' {
                        i += 1;
                    }
                    wipe(&mut o, s, i);
                }
                b'/' if c.get(i + 1) == Some(&b'*') => {
                    let s = i;
                    let mut depth = 0;
                    while i < n {
                        if c[i] == b'/' && c.get(i + 1) == Some(&b'*') {
                            depth += 1;
                            i += 2;
                        } else if c[i] == b'*' && c.get(i + 1) == Some(&b'/') {
                            depth -= 1;
                            i += 2;
                            if depth == 0 {
                                break;
                            }
                        } else {
                            i += 1;
                        }
                    }
                    wipe(&mut o, s, i);
                }
                b'"' => {
                    let s = i;
                    i += 1;
                    while i < n && c[i] != b'"' {
                        i += if c[i] == b'\\' { 2 } else { 1 };
                    }
                    i += 1;
                    wipe(&mut o, s, i);
                }
                b'r' | b'b' if raw_start(c, i).is_some() => {
                    let (hashes, body) = raw_start(c, i).unwrap_or((0, i + 1));
                    let mut j = body;
                    while j < n {
                        if c[j] == b'"' && (0..hashes).all(|h| c.get(j + 1 + h) == Some(&b'#')) {
                            j += 1 + hashes;
                            break;
                        }
                        j += 1;
                    }
                    wipe(&mut o, i, j);
                    i = j;
                }
                b'\'' => {
                    let lit_end = if c.get(i + 1) == Some(&b'\\') {
                        let mut j = i + 3;
                        while j < n && c[j] != b'\'' {
                            j += 1;
                        }
                        Some(j + 1)
                    } else if c.get(i + 1).is_some_and(|b| *b >= 0x80) {
                        let mut j = i + 2;
                        while j < n && c[j] != b'\'' {
                            j += 1;
                        }
                        Some(j + 1)
                    } else if c.get(i + 2) == Some(&b'\'') {
                        Some(i + 3)
                    } else {
                        None
                    };
                    match lit_end {
                        Some(e) => {
                            wipe(&mut o, i, e);
                            i = e;
                        }
                        None => i += 1,
                    }
                }
                _ => i += 1,
            }
        }
        o
    }

    /// If a raw string literal starts at `i` (`r"`, `r#"`, `br"`): its hash
    /// count and the index after the opening quote.
    fn raw_start(c: &[u8], i: usize) -> Option<(usize, usize)> {
        if i > 0 && is_ident(c[i - 1]) {
            return None;
        }
        let mut j = i;
        if c[j] == b'b' {
            j += 1;
        }
        if c.get(j) != Some(&b'r') {
            return None;
        }
        j += 1;
        let mut hashes = 0;
        while c.get(j) == Some(&b'#') {
            hashes += 1;
            j += 1;
        }
        (c.get(j) == Some(&b'"')).then_some((hashes, j + 1))
    }

    fn starts_with_at(c: &[u8], at: usize, pat: &str) -> bool {
        c.get(at..at + pat.len()) == Some(pat.as_bytes())
    }

    /// Blank every `#[cfg(test)]` / `#[test]` item in `c` (an item ends at a
    /// `;` or at the `}` matching its first `{`). Returns the names of
    /// out-of-line test modules (`#[cfg(test)] mod x;`).
    fn strip_tests(c: &mut [u8]) -> Vec<String> {
        let mut out_of_line = Vec::new();
        let mut pos = 0;
        while pos < c.len() {
            let Some(at) = ["#[cfg(test)]", "#[test]"]
                .iter()
                .filter_map(|a| {
                    (pos..c.len())
                        .find(|&k| starts_with_at(c, k, a))
                        .map(|k| (k, a.len()))
                })
                .min()
            else {
                break;
            };
            let (start, alen) = at;
            let mut q = start + alen;
            // further attributes on the same item
            loop {
                while q < c.len() && c[q].is_ascii_whitespace() {
                    q += 1;
                }
                if starts_with_at(c, q, "#[") {
                    let mut d = 0;
                    while q < c.len() {
                        match c[q] {
                            b'[' => d += 1,
                            b']' => {
                                d -= 1;
                                if d == 0 {
                                    q += 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        q += 1;
                    }
                } else {
                    break;
                }
            }
            let item = q;
            // Items whose head may hold a top-level `,` (generics); anything
            // else (a field, a variant, a match arm) ends at `,` or at the
            // parent's closing `}`.
            let rest = String::from_utf8_lossy(&c[item..(item + 40).min(c.len())]).into_owned();
            let rest = strip_vis(rest.trim_start());
            let is_item = [
                "fn ",
                "mod ",
                "struct ",
                "enum ",
                "impl",
                "trait ",
                "use ",
                "const ",
                "static ",
                "type ",
                "unsafe ",
                "async ",
                "extern ",
                "macro_rules!",
            ]
            .iter()
            .any(|k| rest.starts_with(k));
            let mut depth = 0i32;
            let mut end = c.len();
            // a `=>` at depth 0 means a match arm, whose `|` and `>` are
            // operators, not the end of a parameter. A bare `=` is not
            // enough: it also appears in `<Item = u8>` and `<T = u8>`.
            // [A] only `=>` marks an arm; a test-only `let x = |a| ..;`
            // now over-counts, which is the loud direction.
            let mut seen_eq = false;
            while q < c.len() {
                match c[q] {
                    b'=' if depth <= 0 && c.get(q + 1) == Some(&b'>') => seen_eq = true,
                    b'|' if depth <= 0 && !is_item && !seen_eq => {
                        // the closing bar of a closure's parameters
                        // (`|#[cfg(test)] a: u8| { .. }`): the body is not ours
                        end = q;
                        break;
                    }
                    b'>' if depth <= 0
                        && !is_item
                        && !seen_eq
                        && !matches!(c[q - 1], b'=' | b'-') =>
                    {
                        // the closing bracket of a generic list
                        // (`fn f<#[cfg(test)] T>() { .. }`)
                        end = q;
                        break;
                    }
                    b'(' | b'[' => depth += 1,
                    b')' | b']' if depth <= 0 => {
                        // an unmatched closer (the last parameter of a
                        // signature, `fn f(a: u8, #[cfg(test)] b: u8)`)
                        // ends the item: not ours to blank
                        end = q;
                        break;
                    }
                    b')' | b']' => depth -= 1,
                    b';' if depth <= 0 => {
                        end = q + 1;
                        break;
                    }
                    b',' if depth <= 0 && !is_item => {
                        end = q + 1;
                        break;
                    }
                    b'}' if depth <= 0 => {
                        // the parent's closing brace: not ours to blank
                        end = q;
                        break;
                    }
                    b'{' if depth <= 0 => {
                        let mut d = 0;
                        while q < c.len() {
                            match c[q] {
                                b'{' => d += 1,
                                b'}' => {
                                    d -= 1;
                                    if d == 0 {
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            q += 1;
                        }
                        end = (q + 1).min(c.len());
                        break;
                    }
                    _ => {}
                }
                q += 1;
            }
            let head = String::from_utf8_lossy(&c[item..end.min(item + 80)]).into_owned();
            let decl = strip_vis(&head)
                .strip_prefix("mod ")
                .and_then(|rest| rest.split_once(';'))
                .map(|(name, _)| name.trim());
            if let Some(name) = decl.filter(|n| !n.is_empty() && n.bytes().all(is_ident)) {
                out_of_line.push(name.to_owned());
            }
            for x in &mut c[start..end] {
                if *x != b'\n' {
                    *x = b' ';
                }
            }
            pos = end;
        }
        out_of_line
    }

    /// `s` without a leading visibility (`pub `, `pub(crate) `,
    /// `pub(super) `, `pub(in a::b) `).
    fn strip_vis(s: &str) -> &str {
        let s = s.trim_start();
        let Some(r) = s.strip_prefix("pub") else {
            return s;
        };
        if let Some(r) = r.strip_prefix('(') {
            return r.split_once(')').map_or(s, |(_, t)| t.trim_start());
        }
        if r.starts_with(char::is_whitespace) {
            return r.trim_start();
        }
        s
    }

    fn count(code: &str, pat: &str) -> usize {
        if pat == USE_SLEEP {
            // every `use ` at a token boundary (after a block, inside a fn
            // body, after `pub`), up to its `;`
            let word = |ch: char| ch.is_alphanumeric() || ch == '_';
            return code
                .match_indices("use")
                .filter(|(i, _)| code[i + 3..].starts_with(char::is_whitespace))
                .filter(|(i, _)| !code[..*i].ends_with(word))
                .filter(|(i, _)| {
                    let t = code[*i..].split(';').next().unwrap_or("");
                    t.contains("thread") && t.split(|ch: char| !word(ch)).any(|w| w == "sleep")
                })
                .count();
        }
        code.matches(pat).count()
    }

    /// Where an out-of-line module `name` declared in `file` lives.
    fn child_dir(file: &str) -> String {
        let p = Path::new(file);
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let parent = p.parent().and_then(|d| d.to_str()).unwrap_or("");
        let join = |a: &str, b: &str| {
            if a.is_empty() {
                b.to_owned()
            } else {
                format!("{a}/{b}")
            }
        };
        if matches!(stem, "lib" | "main" | "mod") {
            parent.to_owned()
        } else {
            join(parent, stem)
        }
    }

    /// The production-code counts: `file -> pattern -> n`, for the given
    /// `(path under src, source)` pairs. Files that only compile under
    /// `cfg(test)` count nothing.
    fn production_counts(
        files: &[(String, String)],
    ) -> BTreeMap<String, BTreeMap<&'static str, usize>> {
        let mut code: BTreeMap<&str, String> = BTreeMap::new();
        let mut test_only: BTreeSet<String> = BTreeSet::new();
        for (path, src) in files {
            let mut b = blank(src);
            for m in strip_tests(&mut b) {
                let dir = child_dir(path);
                let base = if dir.is_empty() {
                    m
                } else {
                    format!("{dir}/{m}")
                };
                test_only.insert(format!("{base}.rs"));
                test_only.insert(format!("{base}/mod.rs"));
            }
            code.insert(path, String::from_utf8_lossy(&b).into_owned());
        }
        let skip_dirs: Vec<String> = test_only
            .iter()
            .filter(|p| code.contains_key(p.as_str()))
            .map(|p| child_dir(p))
            .collect();
        let mut out = BTreeMap::new();
        for (path, text) in &code {
            let skipped = test_only.contains(*path)
                || skip_dirs
                    .iter()
                    .any(|d| !d.is_empty() && path.starts_with(&format!("{d}/")));
            if skipped || *path == "clock.rs" || path.starts_with("clock/") {
                continue;
            }
            let row: BTreeMap<&'static str, usize> = PATTERNS
                .iter()
                .map(|p| (*p, count(text, p)))
                .filter(|(_, n)| *n > 0)
                .collect();
            if !row.is_empty() {
                out.insert((*path).to_owned(), row);
            }
        }
        out
    }

    /// The lint: every difference between `files` and `sanctioned`.
    fn problems(
        files: &[(String, String)],
        sanctioned: &[(&str, &str, usize, &str)],
    ) -> Vec<String> {
        let found = production_counts(files);
        let mut want: BTreeMap<(&str, &str), usize> = BTreeMap::new();
        for (f, p, n, _) in sanctioned {
            want.insert((*f, *p), *n);
        }
        let mut bad = Vec::new();
        for (f, row) in &found {
            for (p, n) in row {
                let w = want.get(&(f.as_str(), *p)).copied().unwrap_or(0);
                if *n != w {
                    bad.push(format!(
                        "{f}: `{p}` found {n}, sanctioned {w}: use clock::instant() for game time or clock::wall() for real time; see clock_lint.rs"
                    ));
                }
            }
        }
        for ((f, p), w) in &want {
            let n = found.get(*f).and_then(|r| r.get(p)).copied().unwrap_or(0);
            if n == 0 && *w > 0 {
                bad.push(format!(
                    "{f}: `{p}` found 0, sanctioned {w}: lower or drop its SANCTIONED row in clock_lint.rs"
                ));
            }
        }
        bad
    }

    fn read_src(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        paths.sort();
        for p in paths {
            if p.is_dir() {
                read_src(&p, root, out);
            } else if p.extension().is_some_and(|e| e == "rs") {
                let rel = p
                    .strip_prefix(root)
                    .map(|r| r.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                if let Ok(s) = std::fs::read_to_string(&p) {
                    out.push((rel, s));
                }
            }
        }
    }

    fn crate_sources() -> Vec<(String, String)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut v = Vec::new();
        read_src(&root, &root, &mut v);
        v
    }

    #[test]
    fn every_time_source_is_sanctioned() {
        let bad = problems(&crate_sources(), SANCTIONED);
        assert!(bad.is_empty(), "raw time sources:\n{}", bad.join("\n"));
    }

    fn one(path: &str, src: &str) -> Vec<(String, String)> {
        vec![(path.to_owned(), src.to_owned())]
    }

    #[test]
    fn the_lint_can_fail_and_counts_only_production_code() {
        let raw = "fn f() { let _ = std::time::Instant::now(); }\n";
        assert_eq!(problems(&one("x.rs", raw), &[]).len(), 1);
        // sanctioned with the exact count: clean; with another count: not
        let ok = [("x.rs", "Instant::now", 1, "why")];
        assert!(problems(&one("x.rs", raw), &ok).is_empty());
        let stale = [("x.rs", "Instant::now", 2, "why")];
        assert_eq!(problems(&one("x.rs", raw), &stale).len(), 1);
        // comments, strings, chars and test items are not production code
        let quiet = r##"
// Instant::now()
/* SystemTime::now() /* nested */ still a comment */
const S: &str = "Instant::now() {";
const R: &str = r#"thread::sleep( "quoted" "#;
const C: char = '{';
fn lifetime<'a>(x: &'a str) -> &'a str { x }
#[cfg(test)]
mod tests { fn t() { let _ = std::time::Instant::now(); } }
#[test]
fn also_a_test() { let _ = std::time::SystemTime::now(); }
#[cfg(test)]
fn helper() -> [u8; 2] { let _ = std::thread::sleep(d); [0; 2] }
fn production() { let _ = Instant::now(); }
"##;
        let counts = production_counts(&one("y.rs", quiet));
        let row = &counts["y.rs"];
        assert_eq!(row.get("Instant::now"), Some(&1));
        assert_eq!(row.len(), 1, "{row:?}");
    }

    #[test]
    fn a_test_only_field_variant_or_arm_does_not_blank_the_next_item() {
        let cases = [
            "struct S {\n #[cfg(test)]\n probe: u8,\n live: u8,\n}\n",
            "enum E {\n #[cfg(test)]\n Probe,\n Live,\n}\n",
            "fn arm(x: u8) -> u8 { match x {\n #[cfg(test)]\n 0 => 1,\n _ => 2,\n } }\n",
            "struct T {\n live: u8,\n #[cfg(test)]\n last: u8\n}\n",
        ];
        for head in cases {
            let src = format!("{head}fn after() {{ let _ = Instant::now(); }}\n");
            let counts = production_counts(&one("z.rs", &src));
            assert_eq!(counts["z.rs"].get("Instant::now"), Some(&1), "{head}");
        }
        // a test-only generic fn keeps its whole body blanked
        let g = "#[cfg(test)]\nfn generic<A, B>(a: A, b: B) { let _ = Instant::now(); }\n";
        assert!(production_counts(&one("g.rs", g)).is_empty());
    }

    #[test]
    fn an_attribute_on_the_last_parameter_does_not_hide_the_body() {
        let src = "fn f(a: u8, #[cfg(test)] b: u8) { let _ = Instant::now(); }\n\
                   fn g(a: [u8; 2], #[cfg(test)] b: [u8; 1]) { let _ = Instant::now(); }\n";
        let row = &production_counts(&one("p.rs", src))["p.rs"];
        assert_eq!(row.get("Instant::now"), Some(&2), "{row:?}");
    }

    #[test]
    fn a_pub_super_generic_fn_under_cfg_test_is_blanked() {
        let src = "#[cfg(test)]\npub(super) fn g<A, B>(a: A) { let _ = Instant::now(); }\n\
                   fn p() {}\n";
        assert!(production_counts(&one("g.rs", src)).is_empty());
    }

    #[test]
    fn an_attribute_on_a_closure_parameter_does_not_hide_the_body() {
        let src = "fn f() { let c = |#[cfg(test)] a: u8| { let _ = Instant::now(); }; }\n";
        let row = &production_counts(&one("c.rs", src))["c.rs"];
        assert_eq!(row.get("Instant::now"), Some(&1), "{row:?}");
    }

    #[test]
    fn an_attribute_on_a_generic_parameter_does_not_hide_the_body() {
        let src = "fn f<#[cfg(test)] T>() { let _ = Instant::now(); }\n\
                   fn g<#[cfg(test)] T: Fn() -> u8>() { let _ = Instant::now(); }\n";
        let row = &production_counts(&one("g.rs", src))["g.rs"];
        assert_eq!(row.get("Instant::now"), Some(&2), "{row:?}");
    }

    #[test]
    fn an_equals_inside_angle_brackets_does_not_hide_the_body() {
        let srcs = [
            "fn f<#[cfg(test)] T: Iterator<Item = u8>>() { Instant::now() }\n",
            "impl<#[cfg(test)] T: Tr<A = u8>> X for T { fn f() { Instant::now() } }\n",
            "trait Tr<#[cfg(test)] T = u8> { fn f() { Instant::now() } }\n",
            "fn g() { let c = |#[cfg(test)] a: Box<dyn Iterator<Item = u8>>| { Instant::now() }; }\n",
        ];
        for src in srcs {
            let row = &production_counts(&one("e.rs", src))["e.rs"];
            assert_eq!(row.get("Instant::now"), Some(&1), "{src} {row:?}");
        }
    }

    #[test]
    fn a_use_split_over_lines_is_counted() {
        let src = "use\n    std::thread::sleep;\nuse\tstd::thread::{self, sleep};\n";
        let row = &production_counts(&one("w.rs", src))["w.rs"];
        assert_eq!(row.get(USE_SLEEP), Some(&2), "{row:?}");
    }

    #[test]
    fn a_use_after_a_block_or_in_a_body_is_counted() {
        let src = "fn a() {}\nuse std::thread::sleep;\n\
                   fn b() { use std::thread::sleep as nap; nap(d); }\n\
                   mod m { pub(super) use std::thread::sleep; }\n\
                   pub(in crate::x) use std::thread::{self, sleep};\n\
                   fn reuse() {}\nuse std::thread::spawn;\n";
        let row = &production_counts(&one("u.rs", src))["u.rs"];
        assert_eq!(row.get(USE_SLEEP), Some(&4), "{row:?}");
    }

    #[test]
    fn a_test_only_pub_super_or_pub_in_use_is_blanked() {
        let src = "#[cfg(test)]\npub(super) use std::thread::sleep;\n\
                   #[cfg(test)]\npub(in crate::x) use std::thread::sleep;\n\
                   #[cfg(test)]\npub(in crate::x) mod probe;\nfn p() {}\n";
        let files = vec![
            ("a.rs".to_owned(), src.to_owned()),
            (
                "a/probe.rs".to_owned(),
                "fn t() { let _ = Instant::now(); }\n".to_owned(),
            ),
        ];
        assert!(production_counts(&files).is_empty());
    }

    #[test]
    fn imported_and_aliased_time_sources_are_counted() {
        let src = "use std::thread::sleep;\nuse std::thread::{self, sleep as nap};\n\
                   use std::time::{Instant as I, SystemTime as S};\nuse std::thread::spawn;\n\
                   fn f() { let _ = (g.wait_timeout(m, d), TcpStream::connect_timeout(&a, d)); }\n";
        let row = &production_counts(&one("w.rs", src))["w.rs"];
        assert_eq!(row.get(USE_SLEEP), Some(&2), "{row:?}");
        assert_eq!(row.get("Instant as "), Some(&1));
        assert_eq!(row.get("SystemTime as "), Some(&1));
        assert_eq!(row.get("wait_timeout"), Some(&1));
        assert_eq!(row.get("connect_timeout("), Some(&1));
    }

    #[test]
    fn a_test_only_module_file_counts_nothing() {
        let files = vec![
            (
                "a.rs".to_owned(),
                "#[cfg(test)]\nmod probe;\nfn p() {}\n".to_owned(),
            ),
            (
                "a/probe.rs".to_owned(),
                "fn t() { let _ = Instant::now(); }\n".to_owned(),
            ),
            (
                "a/probe/deeper.rs".to_owned(),
                "fn t() { let _ = Instant::now(); }\n".to_owned(),
            ),
        ];
        assert!(production_counts(&files).is_empty());
    }
}
