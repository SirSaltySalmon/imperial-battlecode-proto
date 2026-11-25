//! Small helpers shared by the Markdown and stdout reports.

use crate::stats::Est;
use std::path::Path;

/// A GitHub-flavoured Markdown table.
pub fn md_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut s = format!(
        "| {} |\n|{}\n",
        headers.join(" | "),
        " --- |".repeat(headers.len())
    );
    for row in rows {
        s.push_str(&format!("| {} |\n", row.join(" | ")));
    }
    s
}

/// A p-value that never prints as a misleading "0.0000".
pub fn p_value(p: f64) -> String {
    if p < 0.0001 {
        "< 0.0001".into()
    } else {
        format!("{p:.4}")
    }
}

pub fn pct(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

/// "53.5% [48.1, 58.9]" or "53.5% [no interval]".
pub fn est_pct(e: &Est) -> String {
    match (e.lo, e.hi) {
        (Some(lo), Some(hi)) => format!(
            "{:.1}% [{:.1}, {:.1}]",
            e.value * 100.0,
            lo * 100.0,
            hi * 100.0
        ),
        _ => format!("{:.1}% [no interval]", e.value * 100.0),
    }
}

/// "12.3 [10.1, 14.5]" with the given number of decimals.
pub fn est_num(e: &Est, decimals: usize) -> String {
    match (e.lo, e.hi) {
        (Some(lo), Some(hi)) => format!("{:.d$} [{:.d$}, {:.d$}]", e.value, lo, hi, d = decimals),
        _ => format!("{:.d$} [no interval]", e.value, d = decimals),
    }
}

/// UTC timestamp `YYYYMMDD-HHMMSS` for output directory names (never written inside a report).
pub fn stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = ((secs / 86400) as i64, secs % 86400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (
        doy - (153 * mp + 2) / 5 + 1,
        if mp < 10 { mp + 3 } else { mp - 9 },
    );
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

pub fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn to_json<T: serde::Serialize>(value: &T) -> Result<String, String> {
    let mut s = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    s.push('\n');
    Ok(s)
}
