//! The patch lifecycle (docs/JOBS.md, "Patch boundary"; docs/release.md):
//!
//! - **Switch.** Jobs are pinned to their patch. At a switch, a series that has started (a game running or
//!   finished) and every tournament series finish on the old patch; a series that has not started is
//!   re-planned on the new one (same maps, seeds and sides). What is still queued on the old patch after
//!   `patch_drain_ms` is dead-lettered by the sweeper, except tournament jobs.
//! - **Balance patch.** Every rating's uncertainty is widened (`RatingSystem::on_balance_patch`).
//! - **Revalidation.** Every live submission is checked against the new patch's ABI. One that fails keeps
//!   playing for `invalid_grace_ms`, then its team is left out of ranked pairing until it makes a valid
//!   submission live.
//! - **Language.** The server decides a submission's language and fuel multiplier, never the upload.

use crate::bump;
use crate::config::Config;
use crate::db::patches::Check;
use crate::db::{self, jobs, league, patches, ratings};
use crate::error::{ApiError, ApiResult};
use crate::model::Patch;
use crate::util::{rfc3339, sha256_hex};
use crate::wasmcheck::{check_against, detect_languages, Abi, Limits};
use crate::App;
use axum::body::Bytes;
use axum::http::StatusCode;
use rusqlite::Transaction;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;

/// `POST /v1/admin/patch`, parsed.
pub struct SwitchReq<'a> {
    pub patch: &'a Patch,
    pub runner_blobs: Option<&'a Value>,
    /// The imports the patch offers bots; `None` keeps what was registered (or this build's lists).
    pub abi: Option<&'a Abi>,
    /// A balance patch: widen every rating's uncertainty.
    pub balance: bool,
}

#[derive(Debug, Default)]
pub struct Switched {
    /// False when the patch was already current (runner blobs or ABI updated, nothing else happens).
    pub switched: bool,
    /// Series that had not started, moved onto the new patch.
    pub replanned: usize,
    /// Running series still finishing on an older patch.
    pub kept: i64,
    /// Teams whose rating uncertainty a balance patch widened.
    pub widened: usize,
}

/// Make `req.patch` current and apply the patch boundary.
pub fn switch(app: &App, tx: &Transaction, req: &SwitchReq, now: i64) -> ApiResult<Switched> {
    let previous = patches::current_id(tx)?;
    let id = db::set_current_patch(tx, req.patch, req.runner_blobs, now)?;
    let abi_changed = match req.abi {
        Some(abi) => patches::set_abi(tx, id, abi)?,
        None => false,
    };
    let mut out = Switched {
        switched: previous != Some(id),
        ..Switched::default()
    };
    // Validations on this patch from an earlier time it was current (or under another ABI) are stale: every
    // submission is checked again, and a failure gets a fresh grace period.
    if out.switched || abi_changed {
        patches::forget_checks(tx, id)?;
    }
    if out.switched {
        if let Some(previous) = previous {
            patches::set_superseded(tx, previous, Some(now))?;
        }
        patches::set_superseded(tx, id, None)?;
        let mut cache_hits = 0;
        for series in patches::replannable_series(tx, req.patch)? {
            for job in patches::queued_off_patch(tx, &series, req.patch)? {
                let moved = jobs::repin_queued(tx, &job, &series, req.patch, now)?;
                cache_hits += usize::from(moved.result.is_some());
            }
            out.replanned += 1;
        }
        bump(&app.counters.cache_hits, cache_hits);
        if req.balance {
            let hook = app.rating.as_ref();
            out.widened = ratings::balance_patch(tx, hook, app.cfg.balance_sigma_factor, now)?;
            if out.widened > 0 {
                ratings::write_snapshot(tx, hook, now, app.cfg.ladder_snapshots_kept)?;
            }
        }
    }
    league::finalize_ready(tx, app.rating.as_ref(), now, app.cfg.ladder_snapshots_kept)?;
    out.kept = patches::series_off_patch(tx, req.patch)?;
    Ok(out)
}

/// One sweeper step: dead-letter what superseded patches still have queued after the drain window.
pub fn expire_drained(app: &App, tx: &Transaction, now: i64) -> ApiResult<usize> {
    patches::expire_drained(tx, now - app.cfg.patch_drain_ms, now)
}

fn limits(cfg: &Config) -> Limits {
    Limits {
        max_bytes: cfg.max_wasm_bytes,
        max_memory_bytes: cfg.max_memory_bytes,
    }
}

// ---- revalidation ----

#[derive(Debug, Default)]
pub struct Revalidated {
    pub checked: usize,
    /// `{team_id, submission_id, reason}` of every live submission that fails on the current patch.
    pub failed: Vec<Value>,
}

/// Check every live submission against the current patch's ABI and record the verdicts. A module that
/// cannot be read is skipped (a storage fault is not the team's): it is checked again at the next switch,
/// or when the team makes a submission live.
pub async fn revalidate(app: &Arc<App>) -> ApiResult<Revalidated> {
    let (patch_id, abi, live) = app.db.tx(|tx| {
        let (id, abi) = patches::current_abi(tx)?;
        Ok((id, abi, patches::live_submissions(tx)?))
    })?;
    let Some(patch_id) = patch_id else {
        return Ok(Revalidated::default());
    };
    let (blobs, limits) = (app.blobs.clone(), limits(&app.cfg));
    let verdicts = tokio::task::spawn_blocking(move || {
        let mut out = Vec::with_capacity(live.len());
        for (team_id, submission_id, sha256) in live {
            match blobs.get(&sha256) {
                Ok(Some(wasm)) => {
                    let failure = check_against(&wasm, &limits, &abi).err();
                    out.push((team_id, submission_id, failure));
                }
                Ok(None) => {
                    eprintln!("revalidation: module {sha256} of {submission_id} is missing")
                }
                Err(e) => eprintln!("revalidation: cannot read {sha256}: {e}"),
            }
        }
        out
    })
    .await
    .map_err(ApiError::internal)?;
    let now = crate::util::now_ms();
    app.db.tx(|tx| {
        let mut out = Revalidated::default();
        for (team_id, submission_id, failure) in &verdicts {
            patches::record_check(tx, submission_id, patch_id, failure.as_deref(), now)?;
            out.checked += 1;
            if let Some(reason) = failure {
                out.failed.push(
                    json!({"team_id": team_id, "submission_id": submission_id, "reason": reason}),
                );
            }
        }
        Ok(out)
    })
}

/// Before a submission is made live: check it on the current patch if it was not yet (it was uploaded
/// under another patch), and refuse it if it fails there. `team_id`: only a submission of that team is
/// looked at (anything else is left for `db::set_live` to answer).
pub async fn require_valid(
    app: &Arc<App>,
    submission_id: &str,
    team_id: Option<&str>,
) -> ApiResult<()> {
    let todo = app.db.tx(|tx| {
        let (Some(patch_id), abi) = patches::current_abi(tx)? else {
            return Ok(None);
        };
        let Some(sha256) = patches::validated_module(tx, submission_id, team_id)? else {
            return Ok(None);
        };
        Ok(match patches::current_check(tx, submission_id)? {
            Some(check) => Some(Err(check.failure)),
            None => Some(Ok((patch_id, abi, sha256))),
        })
    })?;
    let failure = match todo {
        None => None,
        Some(Err(failure)) => failure,
        Some(Ok((patch_id, abi, sha256))) => {
            let (blobs, limits) = (app.blobs.clone(), limits(&app.cfg));
            let failure = tokio::task::spawn_blocking(move || match blobs.get(&sha256) {
                Ok(Some(wasm)) => Ok(check_against(&wasm, &limits, &abi).err()),
                Ok(None) => Err(ApiError::internal(format!("module {sha256} is missing"))),
                Err(e) => Err(ApiError::from(e)),
            })
            .await
            .map_err(ApiError::internal)??;
            let now = crate::util::now_ms();
            app.db.tx(|tx| {
                patches::record_check(tx, submission_id, patch_id, failure.as_deref(), now)
            })?;
            failure
        }
    };
    match failure {
        None => Ok(()),
        Some(reason) => Err(invalid_on_patch(submission_id, &reason)),
    }
}

fn invalid_on_patch(submission_id: &str, reason: &str) -> ApiError {
    ApiError::conflict(
        "invalid_on_patch",
        format!("submission {submission_id:?} does not validate on the current patch: {reason}"),
    )
}

/// 409 `patch_changed`: the patch a module was checked on is no longer current.
fn patch_changed() -> ApiError {
    ApiError::conflict(
        "patch_changed",
        "the current patch changed while the module was being checked; send the request again",
    )
}

/// In the transaction that makes a submission of `team_id` live, after `require_valid`: it must have
/// passed on the patch that is current now (the patch may have switched in between).
pub fn confirm_valid(tx: &Transaction, submission_id: &str, team_id: &str) -> ApiResult<()> {
    if patches::current_id(tx)?.is_none()
        || patches::validated_module(tx, submission_id, Some(team_id))?.is_none()
    {
        return Ok(());
    }
    match patches::current_check(tx, submission_id)? {
        Some(Check { failure: None, .. }) => Ok(()),
        Some(Check {
            failure: Some(reason),
            ..
        }) => Err(invalid_on_patch(submission_id, &reason)),
        None => Err(patch_changed()),
    }
}

/// In the transaction that stores an upload: the patch `inspect_upload` checked it on must still be current.
pub fn confirm_same_patch(tx: &Transaction, checked_on: Option<i64>) -> ApiResult<()> {
    if patches::current_id(tx)? == checked_on {
        Ok(())
    } else {
        Err(patch_changed())
    }
}

/// `live_status` (`"ok"`, `"grace"`, `"invalid"`, or null without a live submission), `invalid_reason`,
/// `invalid_since` and `grace_until` of a team, for `GET /v1/team`.
pub fn team_status(app: &App, tx: &Transaction, team_id: &str, now: i64) -> ApiResult<Value> {
    let Some(live) = db::live_submission(tx, team_id)? else {
        return Ok(
            json!({"live_status": null, "invalid_reason": null, "invalid_since": null, "grace_until": null}),
        );
    };
    let check = patches::current_check(tx, &live.id)?;
    let Some((reason, since)) = check.and_then(|c| Some((c.failure?, c.checked_at))) else {
        return Ok(
            json!({"live_status": "ok", "invalid_reason": null, "invalid_since": null, "grace_until": null}),
        );
    };
    let until = since.saturating_add(app.cfg.invalid_grace_ms);
    let status = if now < until { "grace" } else { "invalid" };
    Ok(
        json!({"live_status": status, "invalid_reason": reason, "invalid_since": rfc3339(since),
              "grace_until": rfc3339(until)}),
    )
}

/// Teams left out of ranked pairing: their live submission fails on the current patch and its grace
/// period is over.
pub fn excluded_teams(app: &App, tx: &Transaction, now: i64) -> ApiResult<BTreeSet<String>> {
    patches::excluded_teams(tx, now - app.cfg.invalid_grace_ms)
}

/// Add `current_patch_check: {status: "ok"|"invalid"|"unchecked", reason, checked_at}` to each entry of
/// `GET /v1/submissions`. A submission is checked on a new patch when it is live at the switch, and
/// otherwise when it is made live.
pub fn annotate_submissions(tx: &Transaction, team_id: &str, list: &mut [Value]) -> ApiResult<()> {
    let checks = patches::team_checks(tx, team_id)?;
    for entry in list {
        let check = entry["submission_id"]
            .as_str()
            .and_then(|id| checks.get(id));
        entry["current_patch_check"] = match check {
            None => json!({"status": "unchecked", "reason": null, "checked_at": null}),
            Some(c) => json!({
                "status": if c.failure.is_none() { "ok" } else { "invalid" },
                "reason": c.failure, "checked_at": rfc3339(c.checked_at),
            }),
        };
    }
    Ok(())
}

/// `GET /v1/patch`: the current patch, how many teams are in their grace period or past it, and how many
/// jobs of superseded patches are still draining.
pub fn public_status(app: &App, tx: &Transaction, now: i64) -> ApiResult<Value> {
    let failing = patches::failing_teams(tx)?;
    let cutoff = now - app.cfg.invalid_grace_ms;
    let invalid = failing.iter().filter(|(_, since)| *since <= cutoff).count();
    Ok(json!({
        "patch": db::current_patch(tx)?,
        "teams_in_grace": failing.len() - invalid,
        "teams_invalid": invalid,
        "invalid_grace_secs": app.cfg.invalid_grace_ms / 1000,
        "draining_jobs": patches::draining_jobs(tx)?,
    }))
}

// ---- uploads: validation and language ----

/// What the upload check found, computed off the async threads.
pub struct Inspected {
    pub sha256: String,
    /// Against the current patch's ABI (this build's when no patch is set).
    pub verdict: Result<(), String>,
    /// `wasmcheck::detect_languages`.
    pub detected: BTreeSet<String>,
    /// The patch the verdict is for.
    pub patch_id: Option<i64>,
}

/// Hash, check and store an uploaded module.
pub async fn inspect_upload(app: &Arc<App>, body: Bytes) -> ApiResult<Inspected> {
    let (patch_id, abi) = app.db.tx(patches::current_abi)?;
    let (store, limits) = (app.blobs.clone(), limits(&app.cfg));
    let (sha256, verdict, detected) = tokio::task::spawn_blocking(move || {
        let sha256 = sha256_hex(&body);
        let verdict = check_against(&body, &limits, &abi).and_then(|()| {
            store
                .put(&sha256, &body)
                .map(|_| ())
                .map_err(|e| format!("storage: {e}"))
        });
        (sha256, verdict, detect_languages(&body))
    })
    .await
    .map_err(ApiError::internal)?;
    if let Err(reason) = &verdict {
        if reason.starts_with("storage: ") {
            return Err(ApiError::internal(reason));
        }
    }
    Ok(Inspected {
        sha256,
        verdict,
        detected,
        patch_id,
    })
}

/// 400 naming the languages this server accepts.
pub fn unknown_language(cfg: &Config, language: &str) -> ApiError {
    let names: Vec<&str> = cfg.languages.iter().map(|(n, _)| n.as_str()).collect();
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "unknown_language",
        format!(
            "unknown language {language:?}; this server accepts: {}",
            names.join(", ")
        ),
    )
}

/// The language a submission is entered as, and its fuel multiplier from the server's configuration (an
/// upload never sets it). `declared` (`?language=`) wins; without it, the language the module names if it
/// names exactly one, else `rust`. A module that names languages must name the declared one.
pub fn resolve_language(
    cfg: &Config,
    declared: Option<&str>,
    detected: &BTreeSet<String>,
) -> ApiResult<(String, f64)> {
    let language = match (declared, detected.first()) {
        (Some(l), _) => l.to_string(),
        (None, Some(only)) if detected.len() == 1 => only.clone(),
        (None, _) => "rust".to_string(),
    };
    let fuel_mult = cfg
        .fuel_multiplier(&language)
        .ok_or_else(|| unknown_language(cfg, &language))?;
    if !detected.is_empty() && !detected.contains(&language) {
        let named: Vec<&str> = detected.iter().map(String::as_str).collect();
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "language_mismatch",
            format!(
                "the upload says {language:?} but the module was built from {} (its `ibc.language` or `producers` \
                 section); upload it with ?language={} (the fuel multiplier follows the language)",
                named.join(" + "),
                named[0]
            ),
        ));
    }
    Ok((language, fuel_mult))
}
