//! `ibc-worker`: a thin supervisor that leases match jobs from the coordinator (docs/JOBS.md part B) and runs
//! them through long-lived `ibc run-batch --jsonl` subprocesses (part A). Outbound connections only.

pub mod backoff;
pub mod cache;
pub mod client;
pub mod config;
mod job;
pub mod log;
pub mod protocol;
pub mod runner;
pub mod worker;

pub use config::Config;
pub use worker::{run, Shutdown, Stats};

/// Lower-case hex SHA-256 of a byte slice.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex(&sha2::Sha256::digest(bytes))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// True for exactly 64 lower-case hex digits: the only strings ever used as blob names on disk or in URLs.
pub fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
