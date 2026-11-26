//! `GET /v1/distribution`: the current `ibc` release, its patch and one download per platform with its
//! SHA-256, as written by `cargo xtask dist` (`distribution.json`). `ibc` reads it to tell participants
//! that they are out of date and how to upgrade.
//!
//! The file named by `IBC_DISTRIBUTION` is read on every request, so publishing a release is replacing that
//! file: no restart. It is small and requested at most once a day per participant.

use super::AppState;
use crate::error::{ApiError, ApiResult};
use axum::extract::State;
use axum::Json;
use serde_json::Value;
use std::path::Path;

pub async fn distribution(State(app): AppState) -> ApiResult<Json<Value>> {
    let Some(path) = app.cfg.distribution_file.clone() else {
        return Err(ApiError::not_found(
            "this server publishes no distribution: set IBC_DISTRIBUTION to a distribution.json written by \
             `cargo xtask dist`",
        ));
    };
    let doc = tokio::task::spawn_blocking(move || load(&path))
        .await
        .map_err(ApiError::internal)??;
    Ok(Json(doc))
}

/// Read and check the file: a `version` string and an `artifacts` list of `{target, url, sha256}`.
pub fn load(path: &Path) -> ApiResult<Value> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ApiError::internal(format!("IBC_DISTRIBUTION {}: {e}", path.display())))?;
    let doc: Value = serde_json::from_str(&text)
        .map_err(|e| ApiError::internal(format!("IBC_DISTRIBUTION {}: {e}", path.display())))?;
    check(&doc)
        .map_err(|e| ApiError::internal(format!("IBC_DISTRIBUTION {}: {e}", path.display())))?;
    Ok(doc)
}

fn check(doc: &Value) -> Result<(), String> {
    if !doc["version"].as_str().is_some_and(|v| !v.is_empty()) {
        return Err("`version` must be a non-empty string".into());
    }
    let artifacts = doc["artifacts"]
        .as_array()
        .ok_or("`artifacts` must be a list")?;
    for (i, a) in artifacts.iter().enumerate() {
        let field = |k: &str| a[k].as_str().is_some_and(|v| !v.is_empty());
        let sha_ok = a["sha256"].as_str().is_some_and(crate::util::is_sha256);
        if !(field("target") && field("url") && sha_ok) {
            return Err(format!(
                "artifacts[{i}] needs `target`, `url` and a 64-hex-digit `sha256`"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_distribution_needs_a_version_and_well_formed_artifacts() {
        let sha = "a".repeat(64);
        let good = json!({"version": "0.2.0", "patch": {}, "artifacts": [
            {"target": "x86_64-pc-windows-msvc", "url": "https://x/ibc.zip", "sha256": sha}]});
        assert!(check(&good).is_ok());
        assert!(check(&json!({"version": "0.2.0", "artifacts": []})).is_ok());
        assert!(check(&json!({"artifacts": []})).is_err());
        assert!(check(&json!({"version": "0.2.0"})).is_err());
        let bad_sha =
            json!({"version": "1", "artifacts": [{"target": "t", "url": "u", "sha256": "xyz"}]});
        assert!(check(&bad_sha).unwrap_err().contains("artifacts[0]"));
    }
}
