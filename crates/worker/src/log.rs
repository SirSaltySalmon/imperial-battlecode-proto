//! Structured logs: one JSON object per line on stderr (`ts`, `level`, `msg`, then free fields).
//! `IBC_LOG=debug|info|warn|error` sets the threshold (default `info`).

use serde_json::{Map, Value};
use std::io::Write;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

fn rank(level: &str) -> u8 {
    match level {
        "debug" => 0,
        "info" => 1,
        "warn" => 2,
        _ => 3,
    }
}

fn threshold() -> u8 {
    static T: OnceLock<u8> = OnceLock::new();
    *T.get_or_init(|| {
        std::env::var("IBC_LOG")
            .map(|v| rank(v.trim()))
            .unwrap_or(1)
    })
}

/// RFC 3339 UTC timestamp of `secs` since the Unix epoch.
pub fn rfc3339(secs: u64) -> String {
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from a day count (Howard Hinnant's `civil_from_days`).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn emit(level: &str, msg: &str, fields: &[(&str, Value)]) {
    if rank(level) < threshold() {
        return;
    }
    let mut m = Map::new();
    m.insert("ts".into(), Value::String(rfc3339(unix_now())));
    m.insert("level".into(), Value::String(level.into()));
    m.insert("msg".into(), Value::String(msg.into()));
    for (k, v) in fields {
        m.insert((*k).into(), v.clone());
    }
    // One write per line keeps lines whole across threads.
    let line = format!("{}\n", Value::Object(m));
    let _ = std::io::stderr().lock().write_all(line.as_bytes());
}

/// `log!("info", "leased", jobs = 3, worker = name)`
#[macro_export]
macro_rules! log {
    ($level:literal, $msg:expr $(, $k:ident = $v:expr)* $(,)?) => {
        $crate::log::emit($level, &$msg.to_string(), &[$((stringify!($k), ::serde_json::json!($v))),*])
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn rfc3339_known_dates() {
        assert_eq!(super::rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(super::rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(super::rfc3339(1_767_225_599), "2025-12-31T23:59:59Z");
    }
}
