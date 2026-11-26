//! The patch lifecycle: what a switch does to running series, the drain of the old patch, revalidation
//! with a grace period, balance patches, and the fuel multiplier the server decides from the language.

mod common;

use axum::http::StatusCode;
use common::{bot_wasm, Harness, MockWorker};
use ibc_coordinator::db::ratings;
use serde_json::{json, Value};

/// A patch body on top of the harness's `0.1.0`, with extra fields merged in.
fn patch(engine: &str, extra: Value) -> Value {
    let mut body =
        json!({"engine": engine, "ruleset_hash": "ffeeddccbbaa0099", "wasmtime": "51.0.0"});
    if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        body.extend(extra.clone());
    }
    body
}

/// A bot that imports nothing: valid under any ABI.
fn bare_wasm(tag: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module (memory (export "memory") 1) (func (export "_start")) (data (i32.const 0) "{tag}"))"#
    ))
    .unwrap()
}

/// A valid bot with extra custom sections (`(@producers ...)`, `(@custom ...)`).
fn bot_with(sections: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module (import "ibc" "end_turn" (func $end_turn)) (memory (export "memory") 1)
             (func (export "_start") (call $end_turn)) {sections})"#
    ))
    .unwrap()
}

/// Engine versions of the jobs a series' games point at, in game order.
fn job_engines(h: &Harness, series: &str) -> Vec<String> {
    h.app
        .db
        .tx(|tx| {
            let mut st = tx.prepare(
                "SELECT json_extract(j.payload, '$.patch.engine') FROM games g JOIN jobs j ON j.id = g.job_id
                 WHERE g.series_id = ?1 ORDER BY g.idx",
            )?;
            let rows = st.query_map([series], |r| r.get(0))?;
            Ok(rows.collect::<Result<Vec<String>, _>>()?)
        })
        .unwrap()
}

async fn series(h: &Harness, id: &str) -> Value {
    h.ok("GET", &format!("/v1/series/{id}"), None, Value::Null)
        .await
}

async fn ranked(h: &Harness, a: &str, b: &str) -> String {
    h.admin(
        "POST",
        "/v1/admin/ranked-batch",
        json!({"pairings": [[a, b]]}),
    )
    .await["series"][0]
        .as_str()
        .unwrap()
        .to_string()
}

async fn tournament(h: &Harness, a: &str, b: &str) -> String {
    h.admin(
        "POST",
        "/v1/series",
        json!({"team_a": a, "team_b": b, "kind": "tournament", "games": 2}),
    )
    .await["series_id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn upload(h: &Harness, token: &str, query: &str, wasm: Vec<u8>) -> (StatusCode, Value) {
    let (status, body) = h
        .raw(
            "POST",
            &format!("/v1/submissions{query}"),
            Some(token),
            wasm,
        )
        .await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_switch_keeps_started_series_on_the_old_patch_and_replans_the_rest() {
    let h = Harness::new(|c| c.ranked_games = 2);
    h.league(&["alpha", "beta", "gamma"]).await;
    let started = ranked(&h, "alpha", "beta").await;
    let waiting = ranked(&h, "alpha", "gamma").await;
    let w = MockWorker::register(&h, "w", true).await;
    let first = w.lease_one(&h).await;
    assert_eq!(first["patch"]["engine"], "0.1.0");
    // Created after the lease: tournament jobs are served before anything else.
    let cup = tournament(&h, "beta", "gamma").await;

    // The body is `ibc version --json` as printed, extra fields included.
    let version = json!({"abi": 0, "ibc": "0.2.0", "replay_format": 1});
    let out = h
        .admin("POST", "/v1/admin/patch", patch("0.2.0", version))
        .await;
    assert_eq!(out["switched"], true);
    assert_eq!(out["replanned_series"], 1);
    assert_eq!(out["kept_series"], 2, "{out}");
    assert_eq!(job_engines(&h, &started), ["0.1.0", "0.1.0"]);
    assert_eq!(job_engines(&h, &waiting), ["0.2.0", "0.2.0"]);
    assert_eq!(
        job_engines(&h, &cup),
        ["0.1.0", "0.1.0"],
        "a tournament plays out on its patch"
    );
    let public = h.ok("GET", "/v1/patch", None, Value::Null).await;
    assert_eq!(public["patch"]["engine"], "0.2.0");
    assert_eq!(public["draining_jobs"], 4, "{public}");
    // Nothing was dead-lettered: the re-planned jobs replaced the old ones.
    let dead = h.admin("GET", "/v1/admin/dead", Value::Null).await;
    assert!(dead["jobs"].as_array().unwrap().is_empty(), "{dead}");

    // Workers that still have the old runner finish the started series; everything closes normally.
    assert_eq!(w.complete(&h, &first).await.0, StatusCode::OK);
    assert_eq!(w.drain(&h).await, 5);
    for id in [&started, &waiting] {
        let s = series(&h, id).await;
        assert_eq!(
            (s["state"].as_str(), s["rated"].as_bool()),
            (Some("done"), Some(true)),
            "{s}"
        );
    }
    assert_eq!(series(&h, &cup).await["state"], "done");
    let public = h.ok("GET", "/v1/patch", None, Value::Null).await;
    assert_eq!(public["draining_jobs"], 0);
}

#[tokio::test]
async fn old_jobs_left_after_the_drain_window_are_dead_lettered_except_tournament_ones() {
    let h = Harness::new(|c| {
        c.patch_drain_ms = 0;
        c.ranked_games = 2;
    });
    h.league(&["alpha", "beta", "gamma"]).await;
    let started = ranked(&h, "alpha", "beta").await;
    let w = MockWorker::register(&h, "w", true).await;
    let first = w.lease_one(&h).await;
    let cup = tournament(&h, "beta", "gamma").await;
    h.admin("POST", "/v1/admin/patch", patch("0.2.0", json!({})))
        .await;
    h.app.sweep().unwrap();

    let dead = h.admin("GET", "/v1/admin/dead", Value::Null).await;
    let dead = dead["jobs"].as_array().unwrap();
    assert_eq!(dead.len(), 1, "the started series' second game");
    assert!(dead[0]["error"]
        .as_str()
        .unwrap()
        .starts_with("patch superseded"));
    // The running game still counts.
    assert_eq!(w.complete(&h, &first).await.0, StatusCode::OK);
    let s = series(&h, &started).await;
    assert_eq!(s["state"], "done");
    assert_eq!(
        s["wins_a"].as_i64().unwrap() + s["wins_b"].as_i64().unwrap(),
        1
    );
    // The tournament still waits for a worker of its patch.
    assert_eq!(series(&h, &cup).await["state"], "running");
    assert_eq!(job_engines(&h, &cup), ["0.1.0", "0.1.0"]);
    assert_eq!(w.drain(&h).await, 2);
    assert_eq!(series(&h, &cup).await["state"], "done");
}

#[tokio::test]
async fn a_submission_that_stops_validating_plays_out_its_grace_then_leaves_ranked() {
    let mut h = Harness::new(|c| c.invalid_grace_ms = 3_600_000);
    let teams = h.league(&["alpha", "beta", "gamma"]).await;
    let (alpha, gamma) = (&teams[0].1, &teams[2].1);
    // The new patch no longer offers `end_turn`, which every harness bot imports.
    let out = h
        .admin(
            "POST",
            "/v1/admin/patch",
            patch("0.2.0", json!({"imports": {"ibc": ["count"]}})),
        )
        .await;
    assert_eq!(out["revalidated"], 3);
    assert_eq!(out["invalid_submissions"].as_array().unwrap().len(), 3);
    let me = h.ok("GET", "/v1/team", Some(alpha), Value::Null).await;
    assert_eq!(me["live_status"], "grace", "{me}");
    assert!(me["invalid_reason"]
        .as_str()
        .unwrap()
        .contains("ibc.end_turn"));
    assert!(me["grace_until"].is_string());
    let public = h.ok("GET", "/v1/patch", None, Value::Null).await;
    assert_eq!(
        (
            public["teams_in_grace"].as_i64(),
            public["teams_invalid"].as_i64()
        ),
        (Some(3), Some(0))
    );
    // During the grace period the teams are still paired.
    assert_eq!(h.app.ranked_round(Some(8), 2).unwrap().len(), 1);
    MockWorker::register(&h, "w", true).await.drain(&h).await;

    h.reopen(|c| c.invalid_grace_ms = 0);
    let me = h.ok("GET", "/v1/team", Some(alpha), Value::Null).await;
    assert_eq!(me["live_status"], "invalid");
    assert!(h.app.ranked_round(Some(8), 2).unwrap().is_empty());

    // A module the new patch rejects is refused at upload; one it accepts makes the team valid again.
    let (status, out) = upload(&h, alpha, "", bot_wasm("alpha-2")).await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_wasm"))
    );
    for (token, tag) in [(alpha, "alpha-3"), (gamma, "gamma-3")] {
        let (status, out) = upload(&h, token, "", bare_wasm(tag)).await;
        assert_eq!(status, StatusCode::CREATED, "{out}");
    }
    let me = h.ok("GET", "/v1/team", Some(alpha), Value::Null).await;
    assert_eq!(
        (me["live_status"].as_str(), me["invalid_reason"].as_str()),
        (Some("ok"), None)
    );
    let created = h.app.ranked_round(Some(8), 2).unwrap();
    assert_eq!(created.len(), 1);
    let s = series(&h, &created[0]).await;
    let mut names = [
        s["team_a"]["name"].as_str().unwrap(),
        s["team_b"]["name"].as_str().unwrap(),
    ];
    names.sort();
    assert_eq!(names, ["alpha", "gamma"], "beta is still excluded");

    // Each submission shows its validation on the current patch, and an invalid one cannot go live again.
    let subs = h
        .ok("GET", "/v1/submissions", Some(alpha), Value::Null)
        .await;
    let status_of = |live: bool| -> String {
        subs["submissions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["live"] == live && s["status"] == "validated")
            .map(|s| {
                s["current_patch_check"]["status"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .unwrap()
    };
    assert_eq!(
        (status_of(true), status_of(false)),
        ("ok".to_string(), "invalid".to_string())
    );
    let old = subs["submissions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["live"] == false && s["status"] == "validated")
        .unwrap()["submission_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, out) = h
        .call(
            "POST",
            &format!("/v1/submissions/{old}/live"),
            Some(alpha),
            Value::Null,
        )
        .await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::CONFLICT, Some("invalid_on_patch"))
    );
}

#[tokio::test]
async fn a_submission_uploaded_under_an_older_patch_is_checked_when_made_live() {
    let h = Harness::new(|_| {});
    let teams = h.league(&["alpha", "beta"]).await;
    let alpha = &teams[0].1;
    let (_, kept) = upload(&h, alpha, "?live=false", bot_wasm("alpha-spare")).await;
    h.admin(
        "POST",
        "/v1/admin/patch",
        patch("0.2.0", json!({"imports": {"ibc": ["count"]}})),
    )
    .await;
    let subs = h
        .ok("GET", "/v1/submissions", Some(alpha), Value::Null)
        .await;
    let spare = subs["submissions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["submission_id"] == kept["submission_id"])
        .unwrap()
        .clone();
    assert_eq!(spare["current_patch_check"]["status"], "unchecked");
    let path = format!(
        "/v1/submissions/{}/live",
        kept["submission_id"].as_str().unwrap()
    );
    let (status, _) = h.call("POST", &path, Some(alpha), Value::Null).await;
    assert_eq!(status, StatusCode::CONFLICT);
    // Another team's submission is not this team's business: 404, not a validation verdict.
    let (status, _) = h.call("POST", &path, Some(&teams[1].1), Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Alpha's rating uncertainty on the ladder.
async fn sigma(h: &Harness) -> f64 {
    h.rating("alpha").rating.sigma
}

#[tokio::test]
async fn only_a_balance_patch_widens_rating_uncertainty() {
    let h = Harness::new(|_| {});
    let teams = h.league(&["alpha", "beta"]).await;
    ranked(&h, "alpha", "beta").await;
    MockWorker::register(&h, "w", true).await.drain(&h).await;
    let played = sigma(&h).await;

    let out = h
        .admin("POST", "/v1/admin/patch", patch("0.2.0", json!({})))
        .await;
    assert_eq!(
        (out["balance"].as_bool(), out["ratings_widened"].as_i64()),
        (Some(false), Some(0))
    );
    assert_eq!(sigma(&h).await, played);

    let out = h
        .admin(
            "POST",
            "/v1/admin/patch",
            patch("0.3.0", json!({"balance": true})),
        )
        .await;
    assert_eq!(
        (out["balance"].as_bool(), out["ratings_widened"].as_i64()),
        (Some(true), Some(2))
    );
    let widened = sigma(&h).await;
    assert!(widened > played, "{widened} <= {played}");
    // Setting the same patch again changes nothing, even flagged as a balance patch.
    let out = h
        .admin(
            "POST",
            "/v1/admin/patch",
            patch("0.3.0", json!({"balance": true})),
        )
        .await;
    assert_eq!(
        (out["switched"].as_bool(), out["ratings_widened"].as_i64()),
        (Some(false), Some(0))
    );
    assert_eq!(sigma(&h).await, widened);

    // The widening is in the ledger: rebuilding the ratings from it gives the same numbers.
    let hook = h.app.rating.clone();
    let rebuilt = h
        .app
        .db
        .tx(|tx| {
            ratings::recompute(tx, hook.as_ref(), 0)?;
            ratings::load(tx, hook.as_ref(), &teams[0].0)
        })
        .unwrap();
    assert_eq!(rebuilt.rating.sigma, widened);
}

#[tokio::test]
async fn the_server_decides_the_language_and_its_fuel_multiplier() {
    let h = Harness::new(|c| c.languages = vec![("rust".into(), 1.0), ("python".into(), 3.0)]);
    let teams = h.league(&["alpha", "beta"]).await;
    let alpha = &teams[0].1;

    let (status, out) = upload(&h, alpha, "?language=cobol", bot_wasm("x")).await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("unknown_language"))
    );
    assert!(
        out["message"].as_str().unwrap().contains("rust, python"),
        "{out}"
    );
    let (status, _) = upload(&h, alpha, "?language=python&fuel_mult=10", bot_wasm("x")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, out) = upload(&h, alpha, "?language=python", bot_wasm("alpha-py")).await;
    assert_eq!(status, StatusCode::CREATED, "{out}");
    assert_eq!(
        (out["language"].as_str(), out["fuel_mult"].as_f64()),
        (Some("python"), Some(3.0))
    );
    ranked(&h, "alpha", "beta").await;
    let job = MockWorker::register(&h, "w", true)
        .await
        .lease_one(&h)
        .await;
    let mut mults = [
        job["bot_a"]["fuel_mult"].as_f64().unwrap(),
        job["bot_b"]["fuel_mult"].as_f64().unwrap(),
    ];
    mults.sort_by(f64::total_cmp);
    assert_eq!(mults, [1.0, 3.0], "{job}");

    // A module built by rustc cannot be entered as Python; without a language it is entered as Rust.
    let rustc = bot_with(r#"(@producers (language "Rust" "") (processed-by "rustc" "1.89.0"))"#);
    let (status, out) = upload(&h, alpha, "?language=python", rustc.clone()).await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("language_mismatch"))
    );
    let (status, out) = upload(&h, alpha, "", rustc).await;
    assert_eq!(
        (status, out["language"].as_str()),
        (StatusCode::CREATED, Some("rust"))
    );
    // A toolchain's `ibc.language` section names the language when the upload does not.
    let (status, out) = upload(
        &h,
        alpha,
        "",
        bot_with(r#"(@custom "ibc.language" "python")"#),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{out}");
    assert_eq!(
        (out["language"].as_str(), out["fuel_mult"].as_f64()),
        (Some("python"), Some(3.0))
    );
}

#[tokio::test]
async fn a_worker_without_the_old_runner_stops_being_offered_old_jobs() {
    let h = Harness::new(|c| {
        c.retry_grace_ms = 0;
        c.ranked_games = 2;
    });
    h.league(&["alpha", "beta"]).await;
    let started = ranked(&h, "alpha", "beta").await;
    let (old, new) = (
        MockWorker::register(&h, "old", true).await,
        MockWorker::register(&h, "new", true).await,
    );
    let first = old.lease_one(&h).await;
    h.admin("POST", "/v1/admin/patch", patch("0.2.0", json!({})))
        .await;
    let job = new.lease_one(&h).await;
    assert_eq!(job["patch"]["engine"], "0.1.0");
    assert_eq!(
        new.release(&h, &job, "patch mismatch").await.0,
        StatusCode::OK
    );
    // No retry grace here, yet a job of the patch it lacks is not offered to it again.
    assert!(new.lease(&h, 1, 0).await.is_empty());
    // A worker of the old patch still finishes the series.
    assert_eq!(old.complete(&h, &first).await.0, StatusCode::OK);
    assert_eq!(old.drain(&h).await, 1);
    assert_eq!(series(&h, &started).await["state"], "done");
}

#[tokio::test]
async fn switching_back_to_a_patch_gives_a_failing_submission_a_fresh_grace_period() {
    let h = Harness::new(|c| c.invalid_grace_ms = 3_600_000);
    let teams = h.league(&["alpha", "beta"]).await;
    let alpha = &teams[0].1;
    let strict = patch("0.2.0", json!({"imports": {"ibc": ["count"]}}));
    h.admin("POST", "/v1/admin/patch", strict.clone()).await;
    // Pretend the failure is old: the grace period is over.
    h.app
        .db
        .tx(|tx| {
            Ok(tx.execute(
                "UPDATE submission_checks SET checked_at = checked_at - 7200000",
                [],
            )?)
        })
        .unwrap();
    let me = h.ok("GET", "/v1/team", Some(alpha), Value::Null).await;
    assert_eq!(me["live_status"], "invalid");
    // Setting the same patch again keeps the clock; going back and forward again starts it over.
    h.admin("POST", "/v1/admin/patch", strict.clone()).await;
    let me = h.ok("GET", "/v1/team", Some(alpha), Value::Null).await;
    assert_eq!(me["live_status"], "invalid");
    let original = patch("0.1.0", json!({"ruleset_hash": "00112233aabbccdd"}));
    h.admin("POST", "/v1/admin/patch", original).await;
    let me = h.ok("GET", "/v1/team", Some(alpha), Value::Null).await;
    assert_eq!(me["live_status"], "ok");
    h.admin("POST", "/v1/admin/patch", strict).await;
    let me = h.ok("GET", "/v1/team", Some(alpha), Value::Null).await;
    assert_eq!(me["live_status"], "grace", "{me}");
}
