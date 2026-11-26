//! Moderation: sanctions, rate limits, flags, the audit log, submission retention.

mod common;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use common::{bot_wasm, Harness, MockWorker, ADMIN};
use http_body_util::BodyExt;
use ibc_coordinator::util::{now_ms, sha256_hex};
use serde_json::{json, Value};
use tower::ServiceExt;

/// One request through `router` (which keeps its rate-limit buckets between calls), with extra headers.
async fn send(
    router: &axum::Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (StatusCode, HeaderMap, Value) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let res = router
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let (status, headers) = (res.status(), res.headers().clone());
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn retry_after(headers: &HeaderMap) -> u64 {
    headers["retry-after"].to_str().unwrap().parse().unwrap()
}

#[tokio::test]
async fn a_suspended_team_is_not_paired_cannot_upload_and_comes_back_when_lifted() {
    let h = Harness::new(|_| {});
    let teams = h.league(&["alpha", "beta", "gamma"]).await;
    let (alpha, alpha_token) = &teams[0];

    let out = h
        .admin(
            "POST",
            &format!("/v1/admin/teams/{alpha}/suspend"),
            json!({"reason": "shared a token", "until": "7d"}),
        )
        .await;
    assert_eq!(out["kind"], "suspended");
    assert!(out["until"].is_string());

    // The matchmaker leaves alpha out: beta and gamma are the only pair.
    let batch = h.admin("POST", "/v1/admin/ranked-batch", json!({})).await;
    let series = batch["series"].as_array().unwrap();
    assert_eq!(series.len(), 1);
    let s = h
        .ok(
            "GET",
            &format!("/v1/series/{}", series[0].as_str().unwrap()),
            None,
            Value::Null,
        )
        .await;
    let names = [&s["team_a"]["name"], &s["team_b"]["name"]];
    assert!(!names.contains(&&json!("alpha")), "{s}");

    // Explicit pairings, scrims by the team and against it are refused too.
    let (status, out) = h
        .call(
            "POST",
            "/v1/admin/ranked-batch",
            Some(ADMIN),
            json!({"pairings": [["alpha", "beta"]]}),
        )
        .await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("team_suspended"))
    );
    let (status, out) = h
        .call(
            "POST",
            "/v1/scrims",
            Some(alpha_token),
            json!({"opponent_team_id": "beta"}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(out["reason"], "shared a token");
    let (status, out) = h
        .call(
            "POST",
            "/v1/scrims",
            Some(&teams[1].1),
            json!({"opponent_team_id": "alpha"}),
        )
        .await;
    // The opponent's sanction is private.
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::CONFLICT, Some("opponent_unavailable"))
    );
    assert!(out.get("reason").is_none(), "{out}");

    // Uploads answer 403 with the reason and the end of the suspension; reads still work.
    let (status, body) = h
        .raw(
            "POST",
            "/v1/submissions",
            Some(alpha_token),
            bot_wasm("alpha-2"),
        )
        .await;
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "team_suspended");
    assert!(body["message"].as_str().unwrap().contains("shared a token"));
    assert!(body["until"].is_string());
    h.ok("GET", "/v1/team", Some(alpha_token), Value::Null)
        .await;

    // A ban has no end; `until` is refused.
    let (status, _) = h
        .call(
            "POST",
            &format!("/v1/admin/teams/{alpha}/ban"),
            Some(ADMIN),
            json!({"reason": "x", "until": "1d"}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    h.admin(
        "POST",
        "/v1/admin/teams/alpha/ban",
        json!({"reason": "cheating"}),
    )
    .await;
    let list = h.admin("GET", "/v1/admin/sanctions", Value::Null).await;
    assert_eq!(list["sanctions"][0]["kind"], "banned");
    assert_eq!(list["sanctions"][0]["until"], Value::Null);
    assert_eq!(
        h.admin("GET", "/v1/admin/overview", Value::Null).await["teams"]["sanctioned"],
        1
    );

    let out = h
        .admin(
            "POST",
            &format!("/v1/admin/teams/{alpha}/unban"),
            Value::Null,
        )
        .await;
    assert_eq!(out["lifted"], true);
    let (status, _) = h
        .raw(
            "POST",
            "/v1/submissions",
            Some(alpha_token),
            bot_wasm("alpha-3"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);

    // A suspension that has run out no longer blocks.
    let past = now_ms() - 1;
    h.app
        .db
        .tx(|tx| {
            ibc_coordinator::db::moderation::set_sanction(
                tx,
                alpha,
                "suspended",
                "old",
                Some(past),
                past,
            )
        })
        .unwrap();
    h.ok(
        "POST",
        "/v1/scrims",
        Some(alpha_token),
        json!({"opponent_team_id": "gamma"}),
    )
    .await;
}

#[tokio::test]
async fn over_the_limit_answers_429_with_retry_after() {
    let h = Harness::new(|c| {
        c.moderation.api_per_minute = 3;
        c.moderation.public_per_minute = 2;
        c.moderation.trust_forwarded_for = true;
        c.moderation.uploads_per_hour = 2;
    });
    let router = ibc_coordinator::api::router(h.app.clone());
    let (_, token) = h.team("alpha").await; // one upload

    for _ in 0..3 {
        let (status, _, _) = send(&router, "GET", "/v1/team", Some(&token), &[], vec![]).await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, headers, body) = send(&router, "GET", "/v1/team", Some(&token), &[], vec![]).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"], "rate_limited");
    let secs = retry_after(&headers);
    assert!((1..=20).contains(&secs), "{secs}");
    assert_eq!(body["retry_after"], secs);

    // The admin token and worker routes are not limited.
    for _ in 0..10 {
        let (status, _, _) =
            send(&router, "GET", "/v1/admin/teams", Some(ADMIN), &[], vec![]).await;
        assert_eq!(status, StatusCode::OK);
    }
    let worker = MockWorker::register(&h, "w", false).await;
    for _ in 0..10 {
        let (status, _, _) = send(
            &router,
            "POST",
            "/v1/jobs/lease",
            Some(&worker.token),
            &[],
            json!({"max": 1, "wait_ms": 0}).to_string().into_bytes(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Anonymous requests are limited per client IP.
    let ip = [("x-forwarded-for", "10.0.0.1, 192.0.2.7")];
    for _ in 0..2 {
        let (status, _, _) = send(&router, "GET", "/v1/ladder", None, &ip, vec![]).await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, headers, _) = send(&router, "GET", "/v1/ladder", None, &ip, vec![]).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(retry_after(&headers) >= 1);
    // A made-up token does not get around the IP limit.
    let (status, _, _) = send(
        &router,
        "GET",
        "/v1/ladder",
        Some("ibt_madeup"),
        &ip,
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let other = [("x-forwarded-for", "192.0.2.8")];
    let (status, _, _) = send(&router, "GET", "/v1/ladder", None, &other, vec![]).await;
    assert_eq!(status, StatusCode::OK);
    // Neither does guessing the worker registration secret.
    let reg =
        json!({"registration_token": "guess", "name": "x", "labels": [], "cores": 1}).to_string();
    let (status, _, _) = send(
        &router,
        "POST",
        "/v1/workers/register",
        None,
        &other,
        reg.clone().into_bytes(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = send(
        &router,
        "POST",
        "/v1/workers/register",
        None,
        &other,
        reg.into_bytes(),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

    // Uploads: 2 per team per hour, counted from the submissions table.
    let (status, _) = h
        .raw("POST", "/v1/submissions", Some(&token), bot_wasm("two"))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, headers, body) = send(
        &router,
        "POST",
        "/v1/submissions",
        Some(&token),
        &[],
        bot_wasm("three"),
    )
    .await;
    // This router's token bucket is empty; a fresh router shows the upload limit itself.
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(retry_after(&headers) <= 20, "{body}");
    let (status, body) = h
        .raw("POST", "/v1/submissions", Some(&token), bot_wasm("three"))
        .await;
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert!(body["message"].as_str().unwrap().contains("upload limit"));
    let secs = body["retry_after"].as_u64().unwrap();
    assert!((3500..=3600).contains(&secs), "{secs}");
    // The organisers can still upload for the team.
    let (status, _) = h
        .raw(
            "POST",
            "/v1/submissions?team=alpha",
            Some(ADMIN),
            bot_wasm("three"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn an_oversized_module_gets_a_clear_413() {
    let h = Harness::new(|c| c.max_wasm_bytes = 64);
    let team = h
        .admin("POST", "/v1/admin/teams", json!({"name": "alpha"}))
        .await;
    let token = team["token"].as_str().unwrap();
    let router = ibc_coordinator::api::router(h.app.clone());
    let module = bot_wasm("a module bigger than sixty-four bytes");
    let len = module.len().to_string();
    let (status, _, body) = send(
        &router,
        "POST",
        "/v1/submissions",
        Some(token),
        &[("content-length", &len)],
        module,
    )
    .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::PAYLOAD_TOO_LARGE, Some("too_large"))
    );
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("limit is 64 bytes"),
        "{body}"
    );
}

#[tokio::test]
async fn flags_show_duplicate_modules_bursts_rejections_and_quarantines() {
    let h = Harness::new(|c| c.moderation.burst_flag_uploads = 2);
    let teams = h.league(&["alpha", "beta", "gamma"]).await;
    // beta uploads alpha's module.
    let (status, _) = h
        .raw(
            "POST",
            "/v1/submissions",
            Some(&teams[1].1),
            bot_wasm("alpha"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    // gamma uploads twice more (three in the hour), the last one invalid.
    h.raw(
        "POST",
        "/v1/submissions",
        Some(&teams[2].1),
        bot_wasm("gamma-2"),
    )
    .await;
    let (status, _) = h
        .raw(
            "POST",
            "/v1/submissions",
            Some(&teams[2].1),
            b"not wasm".to_vec(),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let worker = MockWorker::register(&h, "w1", false).await;
    h.admin(
        "POST",
        &format!("/v1/admin/workers/{}/quarantine", worker.id),
        json!({"quarantined": true, "reason": "wrong results"}),
    )
    .await;

    let flags = h.admin("GET", "/v1/admin/flags", Value::Null).await["flags"]
        .as_array()
        .unwrap()
        .clone();
    let of = |kind: &str| -> Vec<Value> {
        flags
            .iter()
            .filter(|f| f["kind"] == kind)
            .cloned()
            .collect()
    };
    let dup = of("duplicate_wasm");
    let mut dup_teams: Vec<&str> = dup
        .iter()
        .map(|f| f["team_name"].as_str().unwrap())
        .collect();
    dup_teams.sort();
    assert_eq!(dup_teams, ["alpha", "beta"], "{flags:?}");
    assert!(dup[0]["detail"]
        .as_str()
        .unwrap()
        .contains(&sha256_hex(&bot_wasm("alpha"))[..12]));
    assert_eq!(of("upload_burst").len(), 1);
    assert_eq!(of("upload_burst")[0]["team_name"], "gamma");
    assert_eq!(of("rejected_upload")[0]["team_name"], "gamma");
    assert_eq!(of("worker_quarantined")[0]["worker"], json!(worker.id));
    assert!(flags.iter().all(|f| f["first_seen"].is_string()));

    // `first_seen` is stable across calls.
    let again = h.admin("GET", "/v1/admin/flags", Value::Null).await;
    let first = |v: &Value| {
        v["flags"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["kind"] == "worker_quarantined")
            .unwrap()["first_seen"]
            .clone()
    };
    assert_eq!(first(&again), of("worker_quarantined")[0]["first_seen"]);
}

#[tokio::test]
async fn the_audit_log_records_admin_writes_and_cannot_be_changed() {
    let h = Harness::new(|_| {});
    let (alpha, _) = h.league(&["alpha"]).await.remove(0);
    h.admin(
        "POST",
        &format!("/v1/admin/teams/{alpha}/suspend"),
        json!({"reason": "spam"}),
    )
    .await;
    h.admin(
        "POST",
        &format!("/v1/admin/teams/{alpha}/unban"),
        json!({"reason": "appealed"}),
    )
    .await;
    let (status, _) = h
        .call(
            "POST",
            "/v1/admin/teams/nobody/ban",
            Some(ADMIN),
            json!({"reason": "x"}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    h.admin("GET", "/v1/admin/overview", Value::Null).await; // reads are not logged
    let sha = h.blob(b"a runner binary").await; // admin writes to worker routes are logged too

    let log = h.admin("GET", "/v1/admin/audit?limit=4", Value::Null).await;
    let e = log["entries"].as_array().unwrap();
    assert_eq!(e.len(), 4);
    assert_eq!(e[0]["action"], format!("PUT /v1/blobs/{sha}"));
    assert_eq!(e[1]["action"], "POST /v1/admin/teams/nobody/ban");
    assert_eq!(e[1]["status"], 404);
    assert_eq!(
        e[2]["action"],
        format!("POST /v1/admin/teams/{alpha}/unban")
    );
    assert_eq!(e[2]["payload"]["reason"], "appealed");
    assert_eq!(e[3]["payload"]["reason"], "spam");
    assert_eq!(e[3]["actor"], "admin");
    // Paging, and the earlier setup (teams, map, patch) is there too.
    let before = e[3]["seq"].as_i64().unwrap();
    let older = h
        .admin(
            "GET",
            &format!("/v1/admin/audit?before={before}"),
            Value::Null,
        )
        .await;
    let actions: Vec<&str> = older["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert!(actions.contains(&"POST /v1/admin/teams"), "{actions:?}");
    assert!(actions.contains(&"PUT /v1/admin/maps/arena"), "{actions:?}");
    // Map bodies are not JSON: only their size is kept.
    assert!(older["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["payload"]["bytes"].is_number()));

    let tamper = h
        .app
        .db
        .tx(|tx| Ok(tx.execute("DELETE FROM admin_audit", [])?));
    assert!(tamper.is_err());
    let tamper = h
        .app
        .db
        .tx(|tx| Ok(tx.execute("UPDATE admin_audit SET actor = 'x'", [])?));
    assert!(tamper.is_err());
}

#[tokio::test]
async fn retention_keeps_the_newest_live_and_pinned_submissions() {
    let h = Harness::new(|c| {
        c.moderation.submissions_kept = 2;
        c.moderation.uploads_per_hour = 0;
    });
    let teams = h.league(&["alpha", "beta"]).await;
    let token = &teams[0].1;
    let first = h.ok("GET", "/v1/team", Some(token), Value::Null).await["live_submission"].clone();
    // A series pins the submission it was created with.
    h.ok(
        "POST",
        "/v1/scrims",
        Some(token),
        json!({"opponent_team_id": "beta"}),
    )
    .await;

    // Uploads of the last hour are never pruned (the upload limit counts them): move these back in time
    // before the last upload.
    let backdate = |h: &Harness| {
        h.app
            .db
            .tx(|tx| {
                Ok(tx.execute(
                    "UPDATE submissions SET created_at = created_at - 7200000",
                    [],
                )?)
            })
            .unwrap();
    };
    let mut ids = Vec::new();
    for tag in ["two", "three", "four", "five", "six"] {
        if tag == "six" {
            let list = h
                .ok("GET", "/v1/submissions", Some(token), Value::Null)
                .await;
            assert_eq!(
                list["submissions"].as_array().unwrap().len(),
                5,
                "nothing pruned yet"
            );
            backdate(&h);
        }
        if tag == "four" {
            // beta also uses module "three": its blob must survive alpha's retention.
            let (status, _) = h
                .raw(
                    "POST",
                    "/v1/submissions",
                    Some(&teams[1].1),
                    bot_wasm("three"),
                )
                .await;
            assert_eq!(status, StatusCode::CREATED);
        }
        let (status, body) = h
            .raw("POST", "/v1/submissions", Some(token), bot_wasm(tag))
            .await;
        assert_eq!(status, StatusCode::CREATED);
        let body: Value = serde_json::from_slice(&body).unwrap();
        ids.push(body["submission_id"].clone());
    }

    let list = h
        .ok("GET", "/v1/submissions", Some(token), Value::Null)
        .await;
    let kept: Vec<Value> = list["submissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["submission_id"].clone())
        .collect();
    assert_eq!(
        kept,
        [ids[4].clone(), ids[3].clone(), first],
        "newest two, then the pinned one: {list}"
    );
    for (tag, exists) in [
        ("alpha", true),
        ("two", false),
        ("three", true),
        ("four", false),
        ("five", true),
        ("six", true),
    ] {
        assert_eq!(
            h.app.blobs.exists(&sha256_hex(&bot_wasm(tag))),
            exists,
            "{tag}"
        );
    }
}

#[tokio::test]
async fn the_overview_counts_queue_workers_and_games() {
    let h = Harness::new(|_| {});
    h.league(&["alpha", "beta"]).await;
    h.admin(
        "POST",
        "/v1/admin/ranked-batch",
        json!({"pairings": [["alpha", "beta"]], "games": 2}),
    )
    .await;
    let worker = MockWorker::register(&h, "w", false).await;
    let job = worker.lease_one(&h).await;
    worker.complete(&h, &job).await;

    let o = h.admin("GET", "/v1/admin/overview", Value::Null).await;
    assert_eq!(
        o["queue"],
        json!([{"class": "ranked", "state": "queued", "jobs": 1, "oldest_age_secs": o["queue"][0]["oldest_age_secs"]}])
    );
    assert_eq!(
        (
            o["workers"]["live"].as_i64(),
            o["workers"]["live_cores"].as_i64()
        ),
        (Some(1), Some(4))
    );
    assert_eq!(o["games"]["last_hour"], 1);
    assert_eq!(o["mean_core_seconds_per_game"], 0.04);
    assert_eq!(o["dead_letter"], 0);
    assert_eq!(o["pause"]["ranked_scheduler"], "every 600 s");
    assert_eq!(o["limits"]["uploads_per_hour"], 20);
    let (status, _) = h
        .call(
            "GET",
            "/v1/admin/overview",
            Some(&worker.token),
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
