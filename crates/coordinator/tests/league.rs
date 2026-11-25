//! Submissions, series, ratings, the ladder, quotas.

mod common;

use axum::http::StatusCode;
use common::{bot_wasm, fake_result, Harness, MockWorker, ADMIN};
use ibc_coordinator::util::sha256_hex;
use ibc_coordinator::wasmcheck::{check_wasm, Limits};
use serde_json::{json, Value};

fn wat(text: &str) -> Vec<u8> {
    wat::parse_str(text).unwrap()
}

#[test]
fn wasm_validation_matches_the_runner_rules() {
    let limits = Limits::default();
    assert_eq!(check_wasm(&bot_wasm("ok"), &limits), Ok(()));

    let env_import = wat(
        r#"(module (import "env" "something" (func)) (import "ibc" "end_turn" (func))
                             (memory (export "memory") 1) (func (export "_start")))"#,
    );
    assert_eq!(
        check_wasm(&env_import, &limits).unwrap_err(),
        "unsupported imports: env.something"
    );
    let unknown_names = wat(
        r#"(module (import "ibc" "cheat" (func)) (import "wasi_snapshot_preview1" "sock_accept" (func))
                                (memory (export "memory") 1) (func (export "_start")))"#,
    );
    assert_eq!(
        check_wasm(&unknown_names, &limits).unwrap_err(),
        "unsupported imports: ibc.cheat, wasi_snapshot_preview1.sock_accept"
    );
    let imported_memory =
        wat(r#"(module (import "ibc" "log" (memory 1)) (func (export "_start")))"#);
    assert!(check_wasm(&imported_memory, &limits)
        .unwrap_err()
        .starts_with("unsupported imports: ibc.log"));

    let no_start = wat(r#"(module (memory (export "memory") 1))"#);
    assert!(check_wasm(&no_start, &limits)
        .unwrap_err()
        .contains("_start"));
    let start_with_params =
        wat(r#"(module (memory (export "memory") 1) (func (export "_start") (param i32)))"#);
    assert!(check_wasm(&start_with_params, &limits)
        .unwrap_err()
        .contains("_start"));
    let no_memory = wat(r#"(module (func (export "_start")))"#);
    assert_eq!(
        check_wasm(&no_memory, &limits).unwrap_err(),
        "missing export `memory`"
    );
    let huge_memory = wat(r#"(module (memory (export "memory") 2000) (func (export "_start")))"#);
    assert!(check_wasm(&huge_memory, &limits)
        .unwrap_err()
        .contains("initial memory"));

    assert!(check_wasm(b"\0asm but not really", &limits)
        .unwrap_err()
        .starts_with("invalid wasm"));
    let type_error =
        wat(r#"(module (memory (export "memory") 1) (func (export "_start") (result i32)))"#);
    assert!(check_wasm(&type_error, &limits).is_err());
    assert!(check_wasm(
        &bot_wasm("ok"),
        &Limits {
            max_bytes: 10,
            ..limits
        }
    )
    .unwrap_err()
    .contains("limit"));
    assert!(check_wasm(b"", &limits).is_err());
}

#[tokio::test]
async fn upload_validates_stores_and_switches_the_live_submission() {
    let h = Harness::new(|_| {});
    let (team_id, token) = h.team("alpha").await;
    let first = sha256_hex(&bot_wasm("alpha"));

    // A module importing `env.something` is rejected with a reason and does not become live.
    let bad = wat(
        r#"(module (import "env" "something" (func)) (memory (export "memory") 1) (func (export "_start")))"#,
    );
    let (status, err) = h.raw("POST", "/v1/submissions", Some(&token), bad).await;
    let err: Value = serde_json::from_slice(&err).unwrap();
    assert_eq!(
        (status, err["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_wasm"))
    );
    assert!(err["message"].as_str().unwrap().contains("env.something"));
    assert_eq!(
        h.ok("GET", "/v1/team", Some(&token), Value::Null).await["team_id"],
        team_id.as_str()
    );

    // A second valid module replaces the first; the organisers can upload on a team's behalf.
    let (status, out) = h
        .raw(
            "POST",
            "/v1/submissions?team=alpha&language=rust",
            Some(ADMIN),
            bot_wasm("alpha v2"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let out: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        (out["live"].as_bool(), out["fuel_mult"].as_f64()),
        (Some(true), Some(1.0))
    );

    let subs = h
        .ok("GET", "/v1/submissions", Some(&token), Value::Null)
        .await;
    let subs = subs["submissions"].as_array().unwrap().clone();
    let brief: Vec<_> = subs
        .iter()
        .map(|s| (s["status"].as_str().unwrap(), s["live"].as_bool().unwrap()))
        .collect();
    assert_eq!(
        brief,
        [
            ("validated", true),
            ("rejected", false),
            ("validated", false)
        ]
    );
    assert_eq!(subs[2]["sha256"], first.as_str());
    assert!(subs[1]["reason"]
        .as_str()
        .unwrap()
        .contains("env.something"));
    // The validated module is a blob workers can fetch.
    let w = MockWorker::register(&h, "w", false).await;
    assert_eq!(
        h.raw("GET", &format!("/v1/blobs/{first}"), Some(&w.token), vec![])
            .await,
        (StatusCode::OK, bot_wasm("alpha"))
    );

    // Unknown language, missing auth, someone else's token class.
    assert_eq!(
        h.raw(
            "POST",
            "/v1/submissions?language=cobol",
            Some(&token),
            bot_wasm("x")
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        h.raw("POST", "/v1/submissions", None, bot_wasm("x"))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        h.raw("POST", "/v1/submissions", Some(&w.token), bot_wasm("x"))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        h.call(
            "POST",
            "/v1/admin/teams",
            Some(&token),
            json!({"name": "beta"})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        h.call(
            "POST",
            "/v1/admin/teams",
            Some(ADMIN),
            json!({"name": "alpha"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}

/// The team whose module hash is smaller wins every fake match.
fn stronger<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
    if sha256_hex(&bot_wasm(a)) < sha256_hex(&bot_wasm(b)) {
        (a, b)
    } else {
        (b, a)
    }
}

#[tokio::test]
async fn a_finished_ranked_series_updates_ratings_and_the_ladder_snapshot() {
    let h = Harness::new(|_| {});
    let empty = h.ok("GET", "/v1/ladder", None, Value::Null).await;
    assert_eq!(empty["teams"], json!([]));
    h.league(&["alpha", "beta"]).await;
    let (winner, loser) = stronger("alpha", "beta");

    let batch = h
        .admin(
            "POST",
            "/v1/admin/ranked-batch",
            json!({"pairings": [["alpha", "beta"]], "games": 4}),
        )
        .await;
    let series_id = batch["series"][0].as_str().unwrap().to_string();
    let series = h
        .ok("GET", &format!("/v1/series/{series_id}"), None, Value::Null)
        .await;
    assert_eq!(
        (series["state"].as_str(), series["games"].as_i64()),
        (Some("running"), Some(4))
    );
    let games = series["matches"].as_array().unwrap();
    // Sides alternate, seeds differ, maps come from the pool.
    for pair in games.windows(2) {
        assert_ne!(pair[0]["bot_a"]["name"], pair[1]["bot_a"]["name"]);
        assert_ne!(pair[0]["seed"], pair[1]["seed"]);
    }
    assert!(games
        .iter()
        .all(|g| g["map"] == "arena" && g["state"] == "queued" && g["winner_team"].is_null()));
    assert_eq!(h.metric("ibc_queue_depth{class=\"ranked\"}").await, 4.0);
    // Teams are on the ladder as soon as they have a live submission: unplayed and provisional.
    let fresh = h.standing("alpha").await;
    assert_eq!(
        (fresh["games"].as_i64(), fresh["provisional"].as_bool()),
        (Some(0), Some(true))
    );

    // Three of four games done: nothing is rated yet.
    let w = MockWorker::register(&h, "w", true).await;
    for job in w.lease(&h, 3, 0).await {
        assert_eq!(w.complete(&h, &job).await.0, StatusCode::OK);
    }
    assert_eq!(h.standing("beta").await["games"], 0);
    let last = w.lease_one(&h).await;
    assert_eq!(w.complete(&h, &last).await.0, StatusCode::OK);

    let series = h
        .ok("GET", &format!("/v1/series/{series_id}"), None, Value::Null)
        .await;
    assert_eq!(
        (series["state"].as_str(), series["rated"].as_bool()),
        (Some("done"), Some(true))
    );
    let (wins_a, wins_b) = if winner == "alpha" { (4, 0) } else { (0, 4) };
    assert_eq!(
        (series["wins_a"].as_i64(), series["wins_b"].as_i64()),
        (Some(wins_a), Some(wins_b))
    );

    let ladder = h.ok("GET", "/v1/ladder", None, Value::Null).await;
    let teams = ladder["teams"].as_array().unwrap();
    assert_eq!(ladder["system"], "weng-lin-bt");
    assert_eq!(
        (teams[0]["name"].as_str(), teams[1]["name"].as_str()),
        (Some(winner), Some(loser))
    );
    assert_eq!(
        (
            teams[0]["rank"].as_i64(),
            teams[0]["wins"].as_i64(),
            teams[0]["games"].as_i64()
        ),
        (Some(1), Some(4), Some(4))
    );
    assert_eq!(
        (teams[1]["losses"].as_i64(), teams[1]["wins"].as_i64()),
        (Some(4), Some(0))
    );
    // The displayed rating is a whole number on the 1500 scale; after four games it is still provisional
    // (and below 1500 for both: it is the conservative estimate mu - 3 sigma).
    let (top, bottom) = (
        teams[0]["rating"].as_i64().unwrap(),
        teams[1]["rating"].as_i64().unwrap(),
    );
    assert!(top > bottom && top < 1500, "{top} {bottom}");
    assert!(teams.iter().all(|t| t["provisional"] == true));
    assert!(teams[0]["mu"].as_f64().unwrap() > 25.0 && teams[1]["mu"].as_f64().unwrap() < 25.0);

    // Matches: listing, detail with the summary, replay download.
    let list = h
        .ok("GET", "/v1/matches?team=alpha&limit=10", None, Value::Null)
        .await;
    let list = list["matches"].as_array().unwrap();
    assert_eq!(list.len(), 4);
    assert!(list.iter().all(|m| m["state"] == "done"
        && m["winner_team"] == teams[0]["team_id"]
        && m["verified"] == true));
    let match_id = format!("{series_id}-1");
    let detail = h
        .ok("GET", &format!("/v1/matches/{match_id}"), None, Value::Null)
        .await;
    assert_eq!(detail["score"].as_array().unwrap().len(), 2);
    assert!(detail["summary"]["outcome"].is_string());
    let (status, replay) = h
        .raw(
            "GET",
            &format!("/v1/matches/{match_id}/replay"),
            None,
            vec![],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        sha256_hex(&replay),
        detail["replay_sha256"].as_str().unwrap()
    );
    assert_eq!(
        h.call("GET", "/v1/matches/nope", None, Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        h.call("GET", "/v1/matches/nope/replay", None, Value::Null)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        h.raw("GET", "/healthz", None, vec![]).await,
        (StatusCode::OK, b"ok\n".to_vec())
    );
}

#[tokio::test]
async fn a_series_waits_for_pending_verification_and_takes_the_trusted_result() {
    let h = Harness::new(|c| c.verify_fraction = 1.0);
    h.league(&["alpha", "beta"]).await;
    let (winner, _) = stronger("alpha", "beta");
    let batch = h
        .admin(
            "POST",
            "/v1/admin/ranked-batch",
            json!({"pairings": [["alpha", "beta"]], "games": 2}),
        )
        .await;
    let path = format!("/v1/series/{}", batch["series"][0].as_str().unwrap());

    let mut cheat = MockWorker::register(&h, "cheat", false).await;
    cheat.cheat = true;
    assert_eq!(cheat.drain(&h).await, 2);
    assert_eq!(
        h.ok("GET", &path, None, Value::Null).await["state"],
        "running",
        "results are provisional"
    );

    let trusted = MockWorker::register(&h, "trusted", true).await;
    while trusted.drain(&h).await > 0 {}
    let series = h.ok("GET", &path, None, Value::Null).await;
    assert_eq!(
        (series["state"].as_str(), series["rated"].as_bool()),
        (Some("done"), Some(true))
    );
    let ladder = h.ok("GET", "/v1/ladder", None, Value::Null).await;
    assert_eq!(
        ladder["teams"][0]["name"], winner,
        "rated from the trusted results, not the tampered ones"
    );
    assert_eq!(ladder["teams"][0]["wins"], 2);
}

#[tokio::test]
async fn unranked_series_respect_the_daily_quota_and_do_not_touch_ratings() {
    let h = Harness::new(|c| c.unranked_daily_games = 3);
    let teams = h.league(&["alpha", "beta", "gamma"]).await;
    let alpha = Some(teams[0].1.as_str());

    let (status, series) = h
        .call(
            "POST",
            "/v1/series",
            alpha,
            json!({"opponent": "beta", "games": 2}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        (
            series["kind"].as_str(),
            series["matches"].as_array().unwrap().len()
        ),
        (Some("unranked"), 2)
    );
    let (status, err) = h
        .call(
            "POST",
            "/v1/series",
            alpha,
            json!({"opponent": "gamma", "games": 2}),
        )
        .await;
    assert_eq!(
        (status, err["error"].as_str()),
        (StatusCode::TOO_MANY_REQUESTS, Some("quota_exceeded"))
    );
    assert_eq!(
        h.call(
            "POST",
            "/v1/series",
            alpha,
            json!({"opponent": &teams[2].0, "games": 1})
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(
        h.call("POST", "/v1/series", alpha, json!({"opponent": "gamma"}))
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    // Quotas are per team; bad requests are rejected before anything is queued.
    let beta = Some(teams[1].1.as_str());
    assert_eq!(
        h.call("POST", "/v1/series", beta, json!({"opponent": "beta"}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        h.call("POST", "/v1/series", beta, json!({"opponent": "nobody"}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        h.call(
            "POST",
            "/v1/series",
            beta,
            json!({"opponent": "alpha", "games": 0})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        h.call(
            "POST",
            "/v1/series",
            beta,
            json!({"opponent": "alpha", "kind": "tournament"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        h.call("POST", "/v1/series", None, json!({"opponent": "alpha"}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        h.call(
            "POST",
            "/v1/series",
            beta,
            json!({"opponent": "alpha", "games": 3})
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(h.metric("ibc_queue_depth{class=\"unranked\"}").await, 6.0);
    // The organisers are not limited and may queue tournament games.
    let (status, t) = h
        .call(
            "POST",
            "/v1/series",
            Some(ADMIN),
            json!({"team_a": "alpha", "team_b": "gamma", "games": 2, "kind": "tournament"}),
        )
        .await;
    assert_eq!(
        (status, t["kind"].as_str()),
        (StatusCode::CREATED, Some("tournament"))
    );

    let w = MockWorker::register(&h, "w", true).await;
    assert_eq!(w.drain(&h).await, 8);
    let done = h
        .ok(
            "GET",
            &format!("/v1/series/{}", series["series_id"].as_str().unwrap()),
            None,
            Value::Null,
        )
        .await;
    assert_eq!(
        (done["state"].as_str(), done["rated"].as_bool()),
        (Some("done"), Some(false))
    );
    assert_eq!(
        done["wins_a"].as_i64().unwrap() + done["wins_b"].as_i64().unwrap(),
        2
    );
    let ladder = h.ok("GET", "/v1/ladder", None, Value::Null).await;
    assert!(
        ladder["teams"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["games"] == 0),
        "unranked and tournament games are not rated: {ladder}"
    );
}

#[tokio::test]
async fn a_new_patch_dead_letters_old_jobs_and_voids_their_series() {
    let h = Harness::new(|_| {});
    h.league(&["alpha", "beta"]).await;
    let batch = h
        .admin(
            "POST",
            "/v1/admin/ranked-batch",
            json!({"pairings": [["alpha", "beta"]]}),
        )
        .await;
    let out = h
        .admin(
            "POST",
            "/v1/admin/patch",
            json!({"engine": "0.2.0", "ruleset_hash": "ffeeddccbbaa0099", "wasmtime": "51.0.0"}),
        )
        .await;
    assert_eq!(out["superseded_jobs"], 2);
    assert_eq!(
        h.ok("GET", "/v1/patch", None, Value::Null).await["patch"]["engine"],
        "0.2.0"
    );
    let series = h
        .ok(
            "GET",
            &format!("/v1/series/{}", batch["series"][0].as_str().unwrap()),
            None,
            Value::Null,
        )
        .await;
    assert_eq!(
        (series["state"].as_str(), series["rated"].as_bool()),
        (Some("void"), Some(false))
    );
    let w = MockWorker::register(&h, "w", true).await;
    assert!(w.lease(&h, 4, 0).await.is_empty());
    // New series run on the new patch.
    h.admin(
        "POST",
        "/v1/admin/ranked-batch",
        json!({"pairings": [["alpha", "beta"]]}),
    )
    .await;
    assert_eq!(w.lease_one(&h).await["patch"]["engine"], "0.2.0");
}

#[tokio::test]
async fn series_need_live_submissions_a_patch_and_a_map_pool() {
    let h = Harness::new(|_| {});
    let pair = json!({"pairings": [["alpha", "beta"]]});
    h.team("alpha").await;
    h.admin("POST", "/v1/admin/teams", json!({"name": "beta"}))
        .await; // no submission
    let (status, err) = h
        .call("POST", "/v1/admin/ranked-batch", Some(ADMIN), pair.clone())
        .await;
    assert_eq!(
        (status, err["error"].as_str()),
        (StatusCode::CONFLICT, Some("no_live_submission"))
    );
    assert_eq!(
        h.raw(
            "POST",
            "/v1/submissions?team=beta",
            Some(ADMIN),
            bot_wasm("beta")
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(
        h.call("POST", "/v1/admin/ranked-batch", Some(ADMIN), pair.clone())
            .await
            .1["error"],
        "no_patch"
    );
    h.set_patch().await;
    assert_eq!(
        h.call("POST", "/v1/admin/ranked-batch", Some(ADMIN), pair.clone())
            .await
            .1["error"],
        "no_maps"
    );
    assert_eq!(
        h.raw(
            "PUT",
            "/v1/admin/maps/spare?in_pool=false",
            Some(ADMIN),
            b"map".to_vec()
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        h.call("POST", "/v1/admin/ranked-batch", Some(ADMIN), pair.clone())
            .await
            .1["error"],
        "no_maps"
    );
    assert_eq!(
        h.raw(
            "PUT",
            "/v1/admin/maps/bad%20name",
            Some(ADMIN),
            b"map".to_vec()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        h.raw("PUT", "/v1/admin/maps/arena", Some(ADMIN), b"map".to_vec())
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        h.ok("GET", "/v1/maps", None, Value::Null).await["maps"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        h.call("POST", "/v1/admin/ranked-batch", Some(ADMIN), pair)
            .await
            .0,
        StatusCode::OK
    );
    // The fake result helper agrees with what the queue hands out.
    let w = MockWorker::register(&h, "w", false).await;
    let job = w.lease_one(&h).await;
    assert_eq!(fake_result(&job, false).1["ticks"], 100);
}

#[tokio::test]
async fn matches_page_without_gaps_and_reuploading_the_live_module_changes_nothing() {
    let h = Harness::new(|_| {});
    let teams = h.league(&["alpha", "beta"]).await;
    h.admin(
        "POST",
        "/v1/admin/ranked-batch",
        json!({"pairings": [["alpha", "beta"]], "games": 5}),
    )
    .await;
    let mut seen = Vec::new();
    let mut path = "/v1/matches?limit=2".to_string();
    loop {
        let page = h.ok("GET", &path, None, Value::Null).await;
        seen.extend(
            page["matches"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m["index"].as_i64().unwrap()),
        );
        match page["next"].as_str() {
            Some(next) => path = format!("/v1/matches?limit=2&before={next}"),
            None => break,
        }
    }
    assert_eq!(seen, [4, 3, 2, 1, 0]);

    // The same bytes again: same submission, and the running series is still rated.
    let before = h
        .ok("GET", "/v1/team", Some(&teams[0].1), Value::Null)
        .await["live_submission"]
        .clone();
    assert_eq!(
        h.raw(
            "POST",
            "/v1/submissions",
            Some(&teams[0].1),
            bot_wasm("alpha")
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(
        h.ok("GET", "/v1/team", Some(&teams[0].1), Value::Null)
            .await["live_submission"],
        before
    );
    let w = MockWorker::register(&h, "w", true).await;
    assert_eq!(w.drain(&h).await, 5);
    assert_eq!(
        h.ok("GET", "/v1/ladder", None, Value::Null).await["teams"][0]["games"],
        5
    );
}
