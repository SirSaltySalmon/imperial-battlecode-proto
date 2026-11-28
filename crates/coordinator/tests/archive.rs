//! The league as the public sees it: per-series ratings, ladder tiers and provisional teams, the private
//! archive (no submission ids or hashes of other teams anywhere public), scrims and pool maps.

mod common;

use axum::http::StatusCode;
use common::{bot_wasm, fake_result, Harness, MockWorker, ADMIN};
use ibc_coordinator::util::sha256_hex;
use serde_json::{json, Value};

async fn upload(h: &Harness, token: &str, tag: &str) -> Value {
    let (status, out) = h
        .raw("POST", "/v1/submissions", Some(token), bot_wasm(tag))
        .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&out)
    );
    serde_json::from_slice(&out).unwrap()
}

async fn ranked(h: &Harness, a: &str, b: &str, games: i64) -> String {
    let out = h
        .admin(
            "POST",
            "/v1/admin/ranked-batch",
            json!({"pairings": [{"a": a, "b": b, "games": games}]}),
        )
        .await;
    out["series"][0].as_str().unwrap().to_string()
}

async fn scrim(h: &Harness, token: &str, body: Value) -> (StatusCode, Value) {
    h.call("POST", "/v1/scrims", Some(token), body).await
}

/// The module hashes the jobs of the queue play, as `(bot_a, bot_b)`, after leasing them all.
async fn leased_bots(h: &Harness, w: &MockWorker) -> Vec<(String, String)> {
    let jobs = w.lease(h, 16, 0).await;
    let bots = jobs
        .iter()
        .map(|j| {
            (
                j["bot_a"]["sha256"].as_str().unwrap().to_string(),
                j["bot_b"]["sha256"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    for job in &jobs {
        assert_eq!(w.complete(h, job).await.0, StatusCode::OK);
    }
    bots
}

#[tokio::test]
async fn a_series_moves_ratings_once_and_the_ladder_shows_tiers_after_placement() {
    let h = Harness::new(|c| c.placement_series = 2);
    h.league(&["alpha", "beta", "gamma"]).await;
    let w = MockWorker::register(&h, "w", true).await;
    ranked(&h, "alpha", "beta", 3).await;
    assert_eq!(w.drain(&h).await, 3);

    // One ledger event for the whole series, listing its three maps.
    let events: Vec<(String, String)> = h
        .app
        .db
        .tx(|tx| {
            let mut st = tx.prepare("SELECT kind, outcome FROM rating_events")?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, "series");
    assert_eq!(events[0].1.split(',').count(), 3);
    assert_eq!(h.rating("alpha").rating.games, 3);

    ranked(&h, "alpha", "beta", 3).await;
    assert_eq!(w.drain(&h).await, 3);
    let ladder = h.ok("GET", "/v1/ladder", None, Value::Null).await;
    let rows = ladder.as_array().unwrap();
    let names: Vec<&str> = rows
        .iter()
        .map(|r| r["team_name"].as_str().unwrap())
        .collect();
    // One order for everyone: the winner, then gamma at the starting 1500 (still placing), then the loser.
    assert_eq!(names[1], "gamma", "{ladder}");
    assert_eq!(
        rows.iter()
            .map(|r| r["rank"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    for row in [&rows[0], &rows[2]] {
        let name = row["team_name"].as_str().unwrap();
        let shown = h.app.rating.display(&h.rating(name).rating);
        assert_eq!(row["provisional"], false);
        assert_eq!(row["rating"], shown.round() as i64);
        assert_eq!(row["tier"], h.app.cfg.tier(shown));
        assert_eq!(row["series"], 2);
    }
    assert_eq!(
        (
            &rows[1]["rating"],
            &rows[1]["tier"],
            &rows[1]["provisional"]
        ),
        (&json!(1500), &Value::Null, &json!(true))
    );
    assert_eq!(rows[1]["games"], 0);

    // Tier bands: Iron below the first floor, Radiant from the last.
    let cfg = &h.app.cfg;
    assert_eq!(cfg.tier(0.0), "Iron");
    assert_eq!(cfg.tier(cfg.tier_floors[0]), "Bronze");
    assert_eq!(cfg.tier(cfg.tier_floors[4] - 0.5), "Platinum");
    assert_eq!(cfg.tier(10_000.0), "Radiant");
}

#[tokio::test]
async fn public_views_never_name_another_teams_submission() {
    let h = Harness::new(|_| {});
    let teams = h.league(&["alpha", "beta"]).await;
    let (alpha, beta) = (&teams[0].1, &teams[1].1);
    let series_id = ranked(&h, "alpha", "beta", 2).await;
    // A worker whose summary names both modules, as the real runner's does.
    let w = MockWorker::register(&h, "w", true).await;
    for job in w.lease(&h, 8, 0).await {
        let (replay, mut result) = fake_result(&job, false);
        result["summary"]["teams"] = json!([
            {"name": "aaaa", "wasm_sha256": job["bot_a"]["sha256"], "faults": 0},
            {"name": "bbbb", "wasm_sha256": job["bot_b"]["sha256"], "faults": 0},
        ]);
        h.blob(&replay).await;
        let path = format!("/v1/jobs/{}/complete", job["job_id"].as_str().unwrap());
        let body = json!({"lease_token": job["lease_token"], "result": result});
        assert_eq!(
            h.call("POST", &path, Some(&w.token), body).await.0,
            StatusCode::OK
        );
    }
    let sub = |token: &str| {
        let token = token.to_string();
        let h = &h;
        async move {
            h.ok("GET", "/v1/team", Some(&token), Value::Null).await["live_submission"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };
    let (sub_a, sub_b) = (sub(alpha).await, sub(beta).await);
    // What identifies a team's module: its submission id, its hash, and the hash prefix the runner names
    // a bot by.
    let secrets_of = |team: &str, sub: &str| {
        let sha = sha256_hex(&bot_wasm(team));
        vec![sub.to_string(), sha[..12].to_string(), sha]
    };
    let (alpha_secrets, beta_secrets) = (secrets_of("alpha", &sub_a), secrets_of("beta", &sub_b));
    let leaks = |body: &Value, forbidden: &[&Vec<String>]| -> Vec<String> {
        let text = body.to_string();
        forbidden
            .iter()
            .flat_map(|s| s.iter())
            .filter(|s| text.contains(s.as_str()))
            .cloned()
            .collect()
    };
    let paths = [
        "/v1/ladder".to_string(),
        "/v1/matches?limit=50".to_string(),
        format!("/v1/matches/{series_id}-0"),
        format!("/v1/series/{series_id}"),
    ];
    for path in &paths {
        let body = h.ok("GET", path, None, Value::Null).await;
        let both = [&alpha_secrets, &beta_secrets];
        assert_eq!(leaks(&body, &both), Vec::<String>::new(), "{path}: {body}");
        let body = h.ok("GET", path, Some(alpha), Value::Null).await;
        assert_eq!(
            leaks(&body, &[&beta_secrets]),
            Vec::<String>::new(),
            "{path}"
        );
    }
    // A team sees its own pinned submission; the organisers see both.
    let mine = h
        .ok(
            "GET",
            &format!("/v1/series/{series_id}"),
            Some(alpha),
            Value::Null,
        )
        .await;
    assert_eq!(mine["team_a"]["submission_id"], sub_a.as_str());
    assert!(mine["team_b"].get("submission_id").is_none());
    let detail = h
        .ok(
            "GET",
            &format!("/v1/matches/{series_id}-0"),
            Some(alpha),
            Value::Null,
        )
        .await;
    let alpha_slot = usize::from(detail["bot_a"]["name"] != "alpha");
    assert!(detail["summary"]["teams"][alpha_slot]["wasm_sha256"].is_string());
    assert!(detail["summary"]["teams"][1 - alpha_slot]["wasm_sha256"].is_null());
    assert_eq!(detail["summary"]["teams"][alpha_slot]["name"], "alpha");
    assert_eq!(detail["summary"]["teams"][1 - alpha_slot]["name"], "beta");
    assert!(detail.get("job_id").is_none());
    let all = h
        .ok(
            "GET",
            &format!("/v1/series/{series_id}"),
            Some(ADMIN),
            Value::Null,
        )
        .await;
    assert_eq!(all["team_b"]["submission_id"], sub_b.as_str());
    let detail = h
        .ok(
            "GET",
            &format!("/v1/matches/{series_id}-0"),
            Some(ADMIN),
            Value::Null,
        )
        .await;
    assert!(detail["job_id"].is_string());
}

#[tokio::test]
async fn a_scrim_reaches_only_the_live_bot_or_the_one_already_played() {
    let h = Harness::new(|_| {});
    let teams = h.league(&["alpha", "beta", "gamma"]).await;
    let (alpha, beta) = (&teams[0].1, &teams[1].1);
    let w = MockWorker::register(&h, "w", true).await;
    let (alpha_v1, beta_v1) = (
        sha256_hex(&bot_wasm("alpha")),
        sha256_hex(&bot_wasm("beta")),
    );
    let played = ranked(&h, "alpha", "beta", 1).await;
    let other = ranked(&h, "beta", "gamma", 1).await;
    w.drain(&h).await;
    let alpha_sub_v1 =
        h.ok("GET", "/v1/team", Some(alpha), Value::Null).await["live_submission"].clone();
    upload(&h, beta, "beta v2").await;
    upload(&h, alpha, "alpha v2").await;
    let (alpha_v2, beta_v2) = (
        sha256_hex(&bot_wasm("alpha v2")),
        sha256_hex(&bot_wasm("beta v2")),
    );
    let pair = |bots: &[(String, String)]| -> Vec<[String; 2]> {
        bots.iter()
            .map(|(a, b)| {
                let mut p = [a.clone(), b.clone()];
                p.sort();
                p
            })
            .collect()
    };
    let sorted = |a: &str, b: &str| {
        let mut p = [a.to_string(), b.to_string()];
        p.sort();
        p
    };

    // Default: both live bots.
    let (status, view) = scrim(&h, alpha, json!({"opponent_team_id": "beta", "games": 2})).await;
    assert_eq!(status, StatusCode::CREATED, "{view}");
    assert!(view["team_b"].get("submission_id").is_none());
    assert_eq!(
        pair(&leased_bots(&h, &w).await),
        vec![sorted(&alpha_v2, &beta_v2); 2]
    );

    // My archived bot against the opponent's bot from a series we played: the server resolves it.
    let body = json!({"opponent_team_id": "beta", "my_submission_id": alpha_sub_v1, "against_series_id": played});
    let (status, view) = scrim(&h, alpha, body).await;
    assert_eq!(status, StatusCode::CREATED, "{view}");
    assert_eq!(view["team_a"]["submission_id"], alpha_sub_v1);
    assert!(!view.to_string().contains(&beta_v1));
    assert_eq!(
        pair(&leased_bots(&h, &w).await),
        vec![sorted(&alpha_v1, &beta_v1)]
    );

    // Only a series both teams played, and only my own submissions.
    for body in [
        json!({"opponent_team_id": "beta", "against_series_id": other}),
        json!({"opponent_team_id": "gamma", "against_series_id": played}),
        json!({"opponent_team_id": "beta", "against_series_id": "snope"}),
    ] {
        assert_eq!(scrim(&h, alpha, body).await.0, StatusCode::NOT_FOUND);
    }
    let beta_sub =
        h.ok("GET", "/v1/team", Some(beta), Value::Null).await["live_submission"].clone();
    let body = json!({"opponent_team_id": "beta", "my_submission_id": beta_sub});
    assert_eq!(scrim(&h, alpha, body).await.0, StatusCode::NOT_FOUND);

    // Maps and sides: pool maps only, in turn; "a" keeps me in engine slot A.
    let body = json!({"opponent_team_id": "gamma", "games": 3, "maps": ["arena"], "sides": "a"});
    let (status, view) = scrim(&h, alpha, body).await;
    assert_eq!(status, StatusCode::CREATED);
    for m in view["matches"].as_array().unwrap() {
        assert_eq!(
            (m["map"].as_str(), m["bot_a"]["name"].as_str()),
            (Some("arena"), Some("alpha"))
        );
    }
    h.raw(
        "PUT",
        "/v1/admin/maps/spare?in_pool=false",
        Some(ADMIN),
        b"spare".to_vec(),
    )
    .await;
    for maps in [json!(["nope"]), json!(["spare"])] {
        let body = json!({"opponent_team_id": "gamma", "maps": maps});
        assert_eq!(scrim(&h, alpha, body).await.0, StatusCode::NOT_FOUND);
    }
    assert_eq!(
        scrim(&h, alpha, json!({"opponent_team_id": "gamma", "maps": []}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    w.drain(&h).await;

    // Listing by kind, and the map text of pool maps only.
    let unranked = h
        .ok("GET", "/v1/matches?kind=unranked", None, Value::Null)
        .await;
    let unranked = unranked["matches"].as_array().unwrap();
    assert_eq!(unranked.len(), 6);
    assert!(unranked.iter().all(|m| m["kind"] == "unranked"));
    let ranked_only = h
        .ok(
            "GET",
            "/v1/matches?kind=ranked&team=gamma",
            None,
            Value::Null,
        )
        .await;
    assert_eq!(ranked_only["matches"].as_array().unwrap().len(), 1);
    assert_eq!(
        h.call("GET", "/v1/matches?kind=bench", None, Value::Null)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        h.raw("GET", "/v1/maps/arena", None, vec![]).await,
        (StatusCode::OK, b"name: arena\n---\n###\n".to_vec())
    );
    assert_eq!(
        h.raw("GET", "/v1/maps/spare", None, vec![]).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_scheduler_starts_by_itself_before_anything_is_measured() {
    // One-second rounds on one 4-core worker: 2.8 core-seconds a round, less than one 3-map series at
    // the 2 s prior.
    let h = Harness::new(|c| {
        c.ranked_interval_ms = 1_000;
        c.ranked_games = 3;
    });
    h.league(&["alpha", "beta"]).await;
    assert!(
        h.app.ranked_round(None, 3).unwrap().is_empty(),
        "no workers"
    );
    let w = MockWorker::register(&h, "w", true).await;
    assert_eq!(h.app.ranked_round(None, 3).unwrap().len(), 1);
    assert_eq!(w.drain(&h).await, 3);
}
