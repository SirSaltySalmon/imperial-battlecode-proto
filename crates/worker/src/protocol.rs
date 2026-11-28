//! Wire types of the coordinator <-> worker lease protocol v1 (docs/JOBS.md part B). Unknown fields are ignored.

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::Value;
use std::collections::HashMap;

/// The identity of the code that runs a match; equals `ibc version --json` of the runner that may serve it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct Patch {
    pub engine: String,
    pub ruleset_hash: String,
    pub wasmtime: String,
}

impl Patch {
    /// `<engine>-<ruleset>-<wasmtime>`, the directory name of a side-by-side runner. `None` if a component
    /// has characters that do not belong in a file name (the coordinator's strings are not trusted as paths).
    pub fn dir_name(&self) -> Option<String> {
        let ok = |s: &str| {
            !s.is_empty()
                && s.len() <= 64
                && !s.starts_with('.')
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
        };
        (ok(&self.engine) && ok(&self.ruleset_hash) && ok(&self.wasmtime))
            .then(|| format!("{}-{}-{}", self.engine, self.ruleset_hash, self.wasmtime))
            .filter(|name| !name.contains(".."))
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct BotRef {
    pub sha256: String,
    /// The language whose `[[language]]` row of the patch's ruleset applies; absent: what the module says.
    #[serde(default)]
    pub language: Option<String>,
    /// Replaces the row's fuel multiplier (organiser bench jobs); absent: the row's.
    #[serde(default)]
    pub fuel_mult: Option<f64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BlobRef {
    pub sha256: String,
}

/// The fields of a `"kind": "match"` job beyond the lease envelope.
#[derive(Clone, Debug, Deserialize)]
pub struct MatchSpec {
    pub patch: Patch,
    pub bot_a: BotRef,
    pub bot_b: BotRef,
    pub map: BlobRef,
    /// u64 as a JSON integer; serde_json parses it exactly, never through a double.
    pub seed: u64,
    #[serde(default)]
    pub max_ticks: Option<u32>,
    /// Optional (docs/JOBS.md): the sha256 of a runner binary for this patch per platform (`"<os>-<arch>"`,
    /// e.g. `"linux-x86_64"`). Only honoured with `--allow-runner-fetch`.
    #[serde(default)]
    pub runner_blobs: Option<HashMap<String, String>>,
}

/// The lease envelope every job carries, whatever its kind.
#[derive(Clone, Debug)]
pub struct Lease {
    pub job_id: String,
    pub lease_token: String,
    pub kind: String,
    /// The lease length the coordinator grants (`lease_secs`), if it says.
    pub lease_secs: Option<f64>,
}

impl Lease {
    pub fn from_value(v: &Value) -> Option<Lease> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Some(Lease {
            job_id: s("job_id")?,
            lease_token: s("lease_token")?,
            kind: s("kind").unwrap_or_default(),
            lease_secs: lease_secs(v),
        })
    }
}

/// A usable `lease_secs` of a job or heartbeat response: positive and finite.
pub fn lease_secs(v: &Value) -> Option<f64> {
    v.get("lease_secs")
        .and_then(Value::as_f64)
        .filter(|s| s.is_finite() && *s > 0.0)
}

#[derive(Serialize)]
pub struct MatchResult<'a> {
    pub winner: &'a Value,
    /// `"a" | "b" | "draw" | "unfinished"`: what ratings are taken from (`winner` is also null when a match
    /// stopped at `max_ticks`).
    pub outcome: &'a Value,
    pub score: &'a Value,
    pub ticks: &'a Value,
    pub replay_sha256: &'a str,
    /// The bytes of `summary.json`, embedded verbatim.
    pub summary: &'a RawValue,
    /// Wall-clock time of the match in the runner: what the coordinator sizes ranked batches by.
    #[serde(skip_serializing_if = "Value::is_null")]
    pub wall_ms: &'a Value,
}

#[derive(Serialize)]
pub struct CompleteBody<'a> {
    pub lease_token: &'a str,
    pub result: MatchResult<'a>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_is_exact_and_unknown_fields_are_ignored() {
        let v: Value = serde_json::from_str(
            r#"{"job_id":"j","lease_token":"t","kind":"match","future":{"x":1},
                "patch":{"engine":"0.1.0","ruleset_hash":"00d2cae8a4f4c61b","wasmtime":"51.0.0","abi":0},
                "bot_a":{"sha256":"a","fuel_mult":2.5},"bot_b":{"sha256":"b","language":"cpp"},"map":{"sha256":"m"},
                "seed":18446744073709551615}"#,
        )
        .unwrap();
        let m: MatchSpec = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(m.seed, u64::MAX);
        assert_eq!(
            (m.bot_a.fuel_mult, m.bot_b.fuel_mult, m.max_ticks),
            (Some(2.5), None, None)
        );
        assert_eq!(
            (m.bot_a.language.as_deref(), m.bot_b.language.as_deref()),
            (None, Some("cpp"))
        );
        assert_eq!(Lease::from_value(&v).unwrap().job_id, "j");
        assert_eq!(m.patch.dir_name().unwrap(), "0.1.0-00d2cae8a4f4c61b-51.0.0");
    }

    #[test]
    fn patch_dir_name_rejects_path_tricks() {
        let p = |e: &str| Patch {
            engine: e.into(),
            ruleset_hash: "ab".into(),
            wasmtime: "1".into(),
        };
        assert!(p("..").dir_name().is_none());
        assert!(p("a/b").dir_name().is_none());
        assert!(p("a\\b").dir_name().is_none());
        assert!(p("").dir_name().is_none());
        assert_eq!(p("0.2.0-rc.1").dir_name().unwrap(), "0.2.0-rc.1-ab-1");
    }
}
