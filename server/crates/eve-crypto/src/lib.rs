//! Cryptographic building blocks, one wrapper module per external crate.
//!
//! This crate is the only place third-party crypto and OS-randomness crates
//! are imported (`server/deps.allow`, the `used-in` column; checked by
//! `tools/check_deps.py`). Our own code stays `forbid(unsafe)`; a dependency
//! may contain `unsafe`.

pub mod rand;
