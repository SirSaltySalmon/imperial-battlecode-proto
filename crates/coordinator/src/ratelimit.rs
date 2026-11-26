//! Abuse limits: the settings of rate limits, retention and moderation flags, and an in-memory token-bucket
//! limiter. Buckets live in the process, so a restart refills them; the upload limit is counted from the
//! submissions table instead (a sliding window that survives restarts, see `db::moderation::check_upload`).

use std::collections::HashMap;
use std::sync::Mutex;

/// Settings of `docs/MODERATION.md`. A limit of 0 turns that limit off.
#[derive(Clone, Debug)]
pub struct ModerationCfg {
    /// Uploads a team may make per rolling hour (rejected uploads count too).
    pub uploads_per_hour: i64,
    /// Requests per minute per team token.
    pub api_per_minute: u32,
    /// Requests per minute per client IP without a team token (anonymous, unknown or other credentials).
    pub public_per_minute: u32,
    /// Take the client IP from the last `X-Forwarded-For` entry (set this behind exactly one proxy).
    pub trust_forwarded_for: bool,
    /// Submissions kept per team: the newest this many, plus every live one and every one a series used.
    pub submissions_kept: i64,
    /// `GET /v1/admin/flags` flags a team with more than this many uploads in the last hour.
    pub burst_flag_uploads: i64,
}

impl Default for ModerationCfg {
    fn default() -> ModerationCfg {
        ModerationCfg {
            uploads_per_hour: 20,
            api_per_minute: 120,
            public_per_minute: 300,
            trust_forwarded_for: false,
            submissions_kept: 200,
            burst_flag_uploads: 10,
        }
    }
}

/// The flags and environment variables of `ModerationCfg`, for `ibc-coordinator`.
#[derive(clap::Args, Clone, Debug)]
pub struct ModerationArgs {
    /// Uploads per team per rolling hour; 0 = unlimited
    #[arg(long, env = "IBC_UPLOADS_PER_HOUR", default_value_t = 20)]
    pub uploads_per_hour: i64,
    /// Requests per team token per minute; 0 = unlimited
    #[arg(long, env = "IBC_API_PER_MINUTE", default_value_t = 120)]
    pub api_per_minute: u32,
    /// Requests per client IP per minute without a team token; 0 = unlimited
    #[arg(long, env = "IBC_PUBLIC_PER_MINUTE", default_value_t = 300)]
    pub public_per_minute: u32,
    /// Trust the last X-Forwarded-For entry as the client IP (behind one reverse proxy)
    #[arg(long, env = "IBC_TRUST_PROXY")]
    pub trust_proxy: bool,
    /// Submissions kept per team besides live ones and ones a series used; 0 = keep all
    #[arg(long, env = "IBC_SUBMISSIONS_KEPT", default_value_t = 200)]
    pub submissions_kept: i64,
    /// Flag a team with more than this many uploads in an hour
    #[arg(long, env = "IBC_UPLOAD_BURST_FLAG", default_value_t = 10)]
    pub upload_burst_flag: i64,
}

impl ModerationArgs {
    pub fn to_config(&self) -> ModerationCfg {
        ModerationCfg {
            uploads_per_hour: self.uploads_per_hour,
            api_per_minute: self.api_per_minute,
            public_per_minute: self.public_per_minute,
            trust_forwarded_for: self.trust_proxy,
            submissions_kept: self.submissions_kept,
            burst_flag_uploads: self.upload_burst_flag,
        }
    }
}

/// What a bucket limits; buckets of different kinds never share tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Requests with a credential, keyed by its hash.
    Credential,
    /// Requests without a known team credential, keyed by client IP.
    Public,
}

#[derive(Clone, Copy, Debug)]
struct Bucket {
    tokens: f64,
    at: i64,
}

/// Token buckets: each holds up to `per_minute` tokens and refills at `per_minute` per minute.
#[derive(Default)]
pub struct Limiter {
    buckets: Mutex<HashMap<(Kind, String), Bucket>>,
}

/// Buckets kept before full (idle) ones are dropped.
const MAX_BUCKETS: usize = 100_000;

impl Limiter {
    fn with_bucket<T>(
        &self,
        kind: Kind,
        key: &str,
        per_minute: u32,
        now: i64,
        f: impl FnOnce(&mut Bucket, f64) -> T,
    ) -> T {
        let cap = f64::from(per_minute);
        let mut map = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() >= MAX_BUCKETS {
            map.retain(|_, b| b.tokens + (now - b.at) as f64 * cap / 60_000.0 < cap);
        }
        let b = map.entry((kind, key.to_string())).or_insert(Bucket {
            tokens: cap,
            at: now,
        });
        b.tokens = (b.tokens + (now - b.at).max(0) as f64 * cap / 60_000.0).min(cap);
        b.at = now;
        f(b, cap)
    }

    /// Seconds until the bucket holds one token again.
    fn wait_secs(tokens: f64, cap: f64) -> u64 {
        ((1.0 - tokens) * 60.0 / cap).ceil().max(1.0) as u64
    }

    /// Take one token. `Err(seconds)` when the bucket is empty. `per_minute == 0` never limits.
    pub fn take(&self, kind: Kind, key: &str, per_minute: u32, now: i64) -> Result<(), u64> {
        if per_minute == 0 {
            return Ok(());
        }
        self.with_bucket(kind, key, per_minute, now, |b, cap| {
            if b.tokens >= 1.0 {
                b.tokens -= 1.0;
                Ok(())
            } else {
                Err(Self::wait_secs(b.tokens, cap))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_empties_and_refills() {
        let l = Limiter::default();
        for _ in 0..3 {
            assert_eq!(l.take(Kind::Credential, "k", 3, 0), Ok(()));
        }
        assert_eq!(l.take(Kind::Credential, "k", 3, 0), Err(20));
        assert_eq!(l.take(Kind::Credential, "k", 3, 10_000), Err(10));
        // Other keys and kinds have their own buckets.
        assert_eq!(l.take(Kind::Credential, "other", 3, 0), Ok(()));
        assert_eq!(l.take(Kind::Public, "k", 3, 0), Ok(()));
        // One token every 20 s.
        assert_eq!(l.take(Kind::Credential, "k", 3, 20_000), Ok(()));
        assert!(l.take(Kind::Credential, "k", 3, 20_000).is_err());
        assert_eq!(l.take(Kind::Credential, "k", 0, 20_000), Ok(()));
    }
}
