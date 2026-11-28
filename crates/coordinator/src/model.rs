//! Wire and storage types of a match job (docs/JOBS.md, part B).

use crate::util::{is_sha256, sha256_hex};
use serde::{Deserialize, Serialize};

/// The patch identity: what `ibc version --json` prints on a worker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Patch {
    pub engine: String,
    pub ruleset_hash: String,
    pub wasmtime: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotRef {
    pub sha256: String,
    /// The bot's language: the runner applies that `[[language]]` row of the patch's ruleset. Absent: what
    /// the module says (`ibc_replay::detect_languages`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Replaces the row's fuel multiplier (organiser bench jobs). Absent: the row's. Submissions never set it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fuel_mult: Option<f64>,
}

impl BotRef {
    /// A submission's module, played with its language's row.
    pub fn submission(sha256: String, language: String) -> BotRef {
        BotRef {
            sha256,
            language: Some(language),
            fuel_mult: None,
        }
    }
}

/// A language name as the ruleset's `[[language]]` rows spell them.
pub fn language_name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapRef {
    pub sha256: String,
}

/// Everything that determines a match. Stored as the job payload and sent to workers as part of `Job`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobSpec {
    pub patch: Patch,
    pub bot_a: BotRef,
    pub bot_b: BotRef,
    pub map: MapRef,
    pub seed: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ticks: Option<u32>,
}

/// Round to the 0.001 resolution of the runner; `None` if out of its range.
pub fn normalize_fuel_mult(m: f64) -> Option<f64> {
    (m.is_finite() && (0.001..=1000.0).contains(&m)).then(|| (m * 1000.0).round() / 1000.0)
}

impl JobSpec {
    /// Check hashes and bring multipliers to their canonical value, so equal matches get equal keys.
    pub fn normalize(mut self) -> Result<JobSpec, String> {
        for (what, sha) in [
            ("bot_a", &self.bot_a.sha256),
            ("bot_b", &self.bot_b.sha256),
            ("map", &self.map.sha256),
        ] {
            if !is_sha256(sha) {
                return Err(format!("{what}.sha256 must be 64 lower-case hex digits"));
            }
        }
        for bot in [&mut self.bot_a, &mut self.bot_b] {
            if let Some(m) = bot.fuel_mult {
                bot.fuel_mult =
                    Some(normalize_fuel_mult(m).ok_or_else(|| {
                        format!("fuel_mult {m} is out of range (0.001 ..= 1000)")
                    })?);
            }
            if let Some(l) = &bot.language {
                if !language_name_ok(l) {
                    return Err(format!("language {l:?} must be 1 to 32 of a-z, 0-9 and _"));
                }
            }
        }
        let p = &self.patch;
        if p.engine.is_empty()
            || p.ruleset_hash.is_empty()
            || [&p.engine, &p.ruleset_hash, &p.wasmtime]
                .iter()
                .any(|s| s.contains('\n'))
        {
            return Err(
                "patch.engine and patch.ruleset_hash must be non-empty single-line strings".into(),
            );
        }
        Ok(self)
    }

    /// The idempotency key of docs/JOBS.md: SHA-256 over the newline-joined inputs, numbers in shortest
    /// decimal form (Rust's `Display` for `f64` and integers: `1`, `1.5`, `12345`), an absent value empty.
    pub fn key(&self) -> String {
        let max_ticks = self.max_ticks.map(|t| t.to_string()).unwrap_or_default();
        let bot = |b: &BotRef| {
            let mult = b.fuel_mult.map(|m| m.to_string()).unwrap_or_default();
            format!(
                "{}\n{mult}\n{}",
                b.sha256,
                b.language.as_deref().unwrap_or("")
            )
        };
        let text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            self.patch.engine,
            self.patch.ruleset_hash,
            self.patch.wasmtime,
            self.map.sha256,
            bot(&self.bot_a),
            bot(&self.bot_b),
            self.seed,
            max_ticks
        );
        sha256_hex(text.as_bytes())
    }
}

/// Priority classes; the number is the wire/storage value (lower is served first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Tournament = 0,
    Ranked = 1,
    Unranked = 2,
    Bench = 3,
}

pub const CLASSES: [Class; 4] = [
    Class::Tournament,
    Class::Ranked,
    Class::Unranked,
    Class::Bench,
];

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::Tournament => "tournament",
            Class::Ranked => "ranked",
            Class::Unranked => "unranked",
            Class::Bench => "bench",
        }
    }

    pub fn parse(s: &str) -> Option<Class> {
        CLASSES.into_iter().find(|c| c.name() == s)
    }

    pub fn from_i64(n: i64) -> Class {
        CLASSES
            .into_iter()
            .find(|c| *c as i64 == n)
            .unwrap_or(Class::Bench)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> JobSpec {
        JobSpec {
            patch: Patch {
                engine: "0.1.0".into(),
                ruleset_hash: "00ff".into(),
                wasmtime: "51.0.0".into(),
            },
            bot_a: BotRef::submission("a".repeat(64), "rust".into()),
            bot_b: BotRef {
                sha256: "b".repeat(64),
                language: None,
                fuel_mult: Some(1.5),
            },
            map: MapRef {
                sha256: "c".repeat(64),
            },
            seed: u64::MAX,
            max_ticks: None,
        }
    }

    #[test]
    fn key_is_the_documented_hash() {
        let text = format!(
            "0.1.0\n00ff\n51.0.0\n{}\n{}\n\nrust\n{}\n1.5\n\n18446744073709551615\n",
            "c".repeat(64),
            "a".repeat(64),
            "b".repeat(64)
        );
        assert_eq!(spec().key(), sha256_hex(text.as_bytes()));
    }

    #[test]
    fn key_depends_on_order_and_cap() {
        let a = spec();
        let mut swapped = spec();
        std::mem::swap(&mut swapped.bot_a, &mut swapped.bot_b);
        assert_ne!(a.key(), swapped.key());
        let mut capped = spec();
        capped.max_ticks = Some(100);
        assert_ne!(a.key(), capped.key());
    }

    #[test]
    fn seed_survives_json_without_a_double() {
        let json = serde_json::to_string(&spec()).unwrap();
        assert!(json.contains("18446744073709551615"));
        assert_eq!(serde_json::from_str::<JobSpec>(&json).unwrap(), spec());
    }

    #[test]
    fn normalize_rounds_and_rejects() {
        let mut s = spec();
        s.bot_a.fuel_mult = Some(1.0004);
        assert_eq!(s.clone().normalize().unwrap().bot_a.fuel_mult, Some(1.0));
        s.bot_a.fuel_mult = Some(0.0);
        assert!(s.clone().normalize().is_err());
        s.bot_a.fuel_mult = None;
        s.bot_a.language = Some("C++".into());
        assert!(s.clone().normalize().is_err());
        s.bot_a.language = None;
        s.map.sha256 = "XYZ".into();
        assert!(s.normalize().is_err());
    }
}
