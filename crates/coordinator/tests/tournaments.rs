//! Tournaments: drafts, the lock snapshot, the bracket run by mock workers, tie-breaks, hidden maps.

mod common;

use axum::http::StatusCode;
use common::{bot_wasm, Harness, MockWorker, ADMIN};
use ibc_coordinator::util::sha256_hex;
use serde_json::{json, Value};

/// A league (patch, pool map `arena`, teams) plus three hidden maps `t1`..`t3`.
async fn setup(h: &Harness, names: &[&str]) -> Vec<(String, String)> {
    let teams = h.league(names).await;
    for map in ["t1", "t2", "t3"] {
        let (status, _) = h
            .raw(
                "PUT",
                &format!("/v1/admin/maps/{map}?in_pool=false"),
                Some(ADMIN),
                format!("name: {map}\n---\n#.#\n").into_bytes(),
            )
            .await;
        assert!(status.is_success());
    }
    teams
}

async fn create(h: &Harness, format: &str, field: Value) -> String {
    let t = h
        .admin(
            "POST",
            "/v1/admin/tournaments",
            json!({"name": "weekly", "format": format, "field": field, "maps": ["t1", "t2", "t3"]}),
        )
        .await;
    assert_eq!(t["status"], "draft");
    t["id"].as_str().unwrap().to_string()
}

async fn act(h: &Harness, id: &str, action: &str) -> Value {
    h.admin(
        "POST",
        &format!("/v1/admin/tournaments/{id}/{action}"),
        Value::Null,
    )
    .await
}

async fn public_view(h: &Harness, id: &str) -> Value {
    h.ok("GET", &format!("/v1/tournaments/{id}"), None, Value::Null)
        .await
}

/// Play every queued job with the mock worker, closing series as they finish, until nothing is left.
async fn run_out(h: &Harness, w: &MockWorker) -> usize {
    let mut played = 0;
    loop {
        let n = w.drain(h).await;
        h.app.sweep().unwrap();
        if n == 0 {
            return played;
        }
        played += n;
    }
}

fn pairings(view: &Value) -> Vec<Value> {
    view["rounds"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| r["pairings"].as_array().unwrap().clone())
        .collect()
}

#[tokio::test]
async fn drafts_are_validated() {
    let h = Harness::new(|_| {});
    setup(&h, &["alpha", "beta"]).await;
    let h = &h;
    let post = |body: Value| async move {
        h.call("POST", "/v1/admin/tournaments", Some(ADMIN), body)
            .await
    };
    let (status, out) = post(json!({"name": "x", "format": "single", "maps": ["t1", "t2"]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");
    assert!(out["message"].as_str().unwrap().contains("odd"));
    let (status, _) = post(json!({"name": "x", "format": "single", "maps": []})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = post(json!({"name": "x", "format": "swiss", "maps": ["t1"]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = post(json!({"name": "x", "format": "single", "maps": ["nope"]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) =
        post(json!({"name": "x", "format": "single", "maps": ["t1", "t1", "t2"]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let field = json!({"kind": "list", "team_ids": ["alpha", "ghost"]});
    let (status, _) =
        post(json!({"name": "x", "format": "single", "field": field, "maps": ["t1"]})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let field = json!({"kind": "top", "n": 1});
    let (status, _) =
        post(json!({"name": "x", "format": "single", "field": field, "maps": ["t1"]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Teams cannot create tournaments.
    let (status, _) = h
        .call(
            "POST",
            "/v1/admin/tournaments",
            None,
            json!({"name": "x", "format": "single", "maps": ["t1"]}),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn lock_snapshots_live_submissions_and_pauses_ranked() {
    let h = Harness::new(|_| {});
    let teams = setup(&h, &["alpha", "beta", "gamma"]).await;
    // A team with no live submission is listed as excluded.
    h.admin("POST", "/v1/admin/teams", json!({"name": "idle"}))
        .await;
    let id = create(&h, "single", json!({"kind": "all"})).await;
    assert!(!h.app.ranked_round(Some(4), 2).unwrap().is_empty());
    h.app.sweep().unwrap();

    let locked = act(&h, &id, "lock").await;
    assert_eq!(locked["status"], "locked");
    assert_eq!(locked["entrants"].as_array().unwrap().len(), 3);
    assert_eq!(locked["excluded"][0]["name"], "idle");
    assert!(locked["ranked_paused_until"].is_string());
    let (status, _) = h
        .call(
            "POST",
            &format!("/v1/admin/tournaments/{id}/lock"),
            Some(ADMIN),
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "a lock happens once");

    // While paused, the ranked scheduler creates nothing (finish the ranked series first so teams are free).
    let w = MockWorker::register(&h, "w", false).await;
    w.drain(&h).await;
    h.app.sweep().unwrap();
    assert!(h.app.ranked_round(Some(4), 2).unwrap().is_empty());

    // beta uploads a new bot after the lock: the tournament still plays the locked one.
    let locked_sub = locked["entrants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "beta")
        .unwrap()["submission_id"]
        .clone();
    let (status, _) = h
        .raw(
            "POST",
            "/v1/submissions",
            Some(&teams[1].1),
            bot_wasm("beta v2"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let started = act(&h, &id, "start").await;
    assert_eq!(started["status"], "running");
    let new_sha = sha256_hex(&bot_wasm("beta v2"));
    let old_sha = sha256_hex(&bot_wasm("beta"));
    let jobs = w.lease(&h, 16, 0).await;
    assert!(!jobs.is_empty());
    for job in &jobs {
        assert_ne!(job["bot_a"]["sha256"], new_sha.as_str());
        assert_ne!(job["bot_b"]["sha256"], new_sha.as_str());
    }
    let beta_played = jobs.iter().any(|j| {
        j["bot_a"]["sha256"] == old_sha.as_str() || j["bot_b"]["sha256"] == old_sha.as_str()
    });
    assert!(beta_played, "seed 2 or 3 is beta, who plays in round 1");
    let subs: Vec<String> = h
        .app
        .db
        .tx(|tx| {
            let mut st = tx.prepare("SELECT sub_a FROM series WHERE kind = 'tournament' UNION SELECT sub_b FROM series WHERE kind = 'tournament'")?;
            let rows = st.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap();
    assert!(subs.contains(&locked_sub.as_str().unwrap().to_string()));
    // The snapshot itself does not move.
    let again = h
        .admin("GET", &format!("/v1/tournaments/{id}"), Value::Null)
        .await;
    assert_eq!(again["entrants"], locked["entrants"]);

    // Resuming ranked play brings the scheduler back while the tournament keeps running.
    act(&h, &id, "resume-ranked").await;
    assert!(!h.app.ranked_round(Some(4), 2).unwrap().is_empty());
}

#[tokio::test]
async fn maps_stay_hidden_until_revealed() {
    let h = Harness::new(|_| {});
    setup(&h, &["alpha", "beta"]).await;
    let names = |v: &Value| -> Vec<String> {
        v["maps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap().to_string())
            .collect()
    };
    let id = create(&h, "single", json!({"kind": "all"})).await;
    let maps = h.ok("GET", "/v1/maps", None, Value::Null).await;
    assert_eq!(names(&maps), vec!["arena"]);
    let view = public_view(&h, &id).await;
    assert!(view["maps"].is_null());
    assert_eq!(view["map_count"], 3);
    let admin_view = h
        .admin("GET", &format!("/v1/tournaments/{id}"), Value::Null)
        .await;
    assert_eq!(admin_view["maps"], json!(["t1", "t2", "t3"]));

    act(&h, &id, "lock").await;
    act(&h, &id, "start").await;
    let w = MockWorker::register(&h, "w", false).await;
    run_out(&h, &w).await;
    let view = public_view(&h, &id).await;
    assert_eq!(view["status"], "finished");
    assert!(view["maps"].is_null());
    // The games are not listed, nor reachable, until the maps are revealed.
    let matches = h.ok("GET", "/v1/matches", None, Value::Null).await;
    assert_eq!(matches["matches"], json!([]));
    let series = pairings(&view)[0]["series_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, _) = h
        .call("GET", &format!("/v1/series/{series}"), None, Value::Null)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = h
        .call(
            "GET",
            &format!("/v1/matches/{series}-0/replay"),
            None,
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let revealed = act(&h, &id, "reveal").await;
    assert!(revealed["revealed_at"].is_string());
    let view = public_view(&h, &id).await;
    assert_eq!(view["maps"], json!(["t1", "t2", "t3"]));
    let maps = h.ok("GET", "/v1/maps", None, Value::Null).await;
    assert_eq!(names(&maps), vec!["arena", "t1", "t2", "t3"]);
    assert!(maps["maps"]
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["in_pool"] == true));
    let matches = h.ok("GET", "/v1/matches", None, Value::Null).await;
    assert_eq!(matches["matches"].as_array().unwrap().len(), 3);
    h.ok("GET", &format!("/v1/series/{series}"), None, Value::Null)
        .await;
    let (status, _) = h
        .call(
            "POST",
            &format!("/v1/admin/tournaments/{id}/reveal"),
            Some(ADMIN),
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn double_elimination_runs_to_a_champion() {
    let h = Harness::new(|_| {});
    let names = ["alpha", "beta", "gamma", "delta", "eps"];
    setup(&h, &names).await;
    let id = create(&h, "double", json!({"kind": "all"})).await;
    act(&h, &id, "lock").await;
    let started = act(&h, &id, "start").await;
    // 5 entrants: an 8-slot bracket with three byes in winners round 1.
    let first = &started["rounds"][0];
    assert_eq!(first["stage"], "winners");
    let states: Vec<&str> = first["pairings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, vec!["bye", "running", "bye", "bye"]);
    assert_eq!(first["pairings"][1]["seed_a"], 4);
    assert_eq!(first["pairings"][1]["seed_b"], 5);

    let w = MockWorker::register(&h, "w", false).await;
    let games = run_out(&h, &w).await;
    let view = public_view(&h, &id).await;
    assert_eq!(view["status"], "finished", "{view}");
    // The mock gives every map to the bot with the smaller hash: that team wins everything 3-0.
    let best = names
        .iter()
        .min_by_key(|n| sha256_hex(&bot_wasm(n)))
        .unwrap();
    let champion = view["entrants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == *best)
        .unwrap()["team_id"]
        .clone();
    assert_eq!(view["winner"], champion);
    let played: Vec<Value> = pairings(&view)
        .into_iter()
        .filter(|p| p["state"] == "done")
        .collect();
    // Without a bracket reset, 2n - 3 or 2n - 2 series, one game per map.
    assert!(played.len() == 7 || played.len() == 8, "{}", played.len());
    assert_eq!(games, played.len() * 3);
    for p in &played {
        assert_eq!(p["decided_by"], "maps");
        let maps = p["maps_won"].as_array().unwrap();
        assert_eq!(maps[0].as_f64().unwrap() + maps[1].as_f64().unwrap(), 3.0);
        assert_eq!(p["rounds_won"].as_array().unwrap().len(), 2);
    }
    let last = view["rounds"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["stage"], "final");
    // Tournament series are not rated.
    let alpha = h.standing("alpha").await;
    assert_eq!(alpha["games"], 0);
    let list = h.ok("GET", "/v1/tournaments", None, Value::Null).await;
    assert_eq!(list["tournaments"][0]["id"], id.as_str());
    assert_eq!(list["tournaments"][0]["entrants"], 5);
}

/// Complete one leased job with a chosen outcome and score (by engine slot).
async fn complete_with(h: &Harness, w: &MockWorker, job: &Value, outcome: &str, score: [i64; 2]) {
    let replay = format!("custom|{}", job["job_id"]).into_bytes();
    let sha = sha256_hex(&replay);
    let (status, _) = h
        .raw("PUT", &format!("/v1/blobs/{sha}"), Some(&w.token), replay)
        .await;
    assert!(status.is_success());
    let winner = match outcome {
        "a" => json!(0),
        "b" => json!(1),
        _ => Value::Null,
    };
    let result = json!({"winner": winner, "outcome": outcome, "score": score, "ticks": 100,
                        "replay_sha256": sha, "summary": {"outcome": outcome}});
    let (status, out) = h
        .call(
            "POST",
            &format!("/v1/jobs/{}/complete", job["job_id"].as_str().unwrap()),
            Some(&w.token),
            json!({"lease_token": job["lease_token"], "result": result}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
}

/// Run a two-team tournament where every map is a draw with the given score; returns its only pairing.
async fn all_draws(h: &Harness, w: &MockWorker, score: [i64; 2]) -> Value {
    let id = create(
        h,
        "single",
        json!({"kind": "list", "team_ids": ["alpha", "beta"]}),
    )
    .await;
    act(h, &id, "lock").await;
    act(h, &id, "start").await;
    for job in w.lease(h, 16, 0).await {
        complete_with(h, w, &job, "draw", score).await;
    }
    h.app.sweep().unwrap();
    let view = public_view(h, &id).await;
    assert_eq!(view["status"], "finished");
    pairings(&view).remove(0)
}

#[tokio::test]
async fn level_pairings_go_to_rounds_then_seed() {
    let h = Harness::new(|_| {});
    let teams = setup(&h, &["alpha", "beta"]).await;
    let w = MockWorker::register(&h, "w", false).await;

    // Level on maps and rounds: the higher seed (alpha, first by name among equal ratings) advances.
    let p = all_draws(&h, &w, [6, 6]).await;
    assert_eq!(p["decided_by"], "seed");
    assert_eq!(p["maps_won"], json!([1.5, 1.5]));
    assert_eq!(p["rounds_won"], json!([18, 18]));
    assert_eq!(p["winner"], teams[0].0.as_str());
    assert_eq!(p["seed_a"], 1);

    // Level on maps, not on rounds: the engine slot with more rounds alternates between the teams, so
    // with three maps one team won 2 x 7 + 5 and the other 2 x 5 + 7.
    let p = all_draws(&h, &w, [7, 5]).await;
    assert_eq!(p["decided_by"], "rounds");
    let rounds = p["rounds_won"].as_array().unwrap();
    let (a, b) = (rounds[0].as_i64().unwrap(), rounds[1].as_i64().unwrap());
    assert_eq!(a + b, 36);
    assert_ne!(a, b);
    let expected = if a > b { &p["a"] } else { &p["b"] };
    assert_eq!(&p["winner"], expected);
}

#[tokio::test]
async fn a_patch_change_replays_the_pairing() {
    let h = Harness::new(|_| {});
    setup(&h, &["alpha", "beta"]).await;
    let id = create(&h, "single", json!({"kind": "all"})).await;
    act(&h, &id, "lock").await;
    let started = act(&h, &id, "start").await;
    let first_series = pairings(&started)[0]["series_id"].clone();
    h.admin(
        "POST",
        "/v1/admin/patch",
        json!({"engine": "0.2.0", "ruleset_hash": "00112233aabbccdd", "wasmtime": "51.0.0"}),
    )
    .await;
    let view = public_view(&h, &id).await;
    let p = pairings(&view).remove(0);
    assert_eq!(p["state"], "running");
    assert_ne!(p["series_id"], first_series, "replayed as a new series");
    let w = MockWorker::register(&h, "w", false).await;
    let jobs = w.lease(&h, 16, 0).await;
    assert_eq!(jobs.len(), 3);
    assert!(jobs.iter().all(|j| j["patch"]["engine"] == "0.2.0"));
    for job in &jobs {
        w.complete(&h, job).await;
    }
    h.app.sweep().unwrap();
    let view = public_view(&h, &id).await;
    assert_eq!(view["status"], "finished");
    assert_eq!(pairings(&view)[0]["decided_by"], "maps");
}

#[tokio::test]
async fn games_that_cannot_run_are_replayed_then_decided() {
    let h = Harness::new(|_| {});
    let teams = setup(&h, &["alpha", "beta"]).await;
    let id = create(&h, "single", json!({"kind": "all"})).await;
    act(&h, &id, "lock").await;
    act(&h, &id, "start").await;
    let w = MockWorker::register(&h, "w", false).await;
    let mut series = Vec::new();
    // Every game fails for good: the pairing is played three times in all, then decided on nothing.
    for _ in 0..3 {
        let view = public_view(&h, &id).await;
        let p = pairings(&view).remove(0);
        assert_eq!(p["state"], "running");
        series.push(p["series_id"].clone());
        let jobs = w.lease(&h, 16, 0).await;
        assert_eq!(jobs.len(), 3);
        for job in &jobs {
            let (status, _) = w.fail(&h, job, false).await;
            assert_eq!(status, StatusCode::OK);
        }
        h.app.sweep().unwrap();
    }
    series.dedup();
    assert_eq!(series.len(), 3);
    let view = public_view(&h, &id).await;
    assert_eq!(view["status"], "finished");
    let p = pairings(&view).remove(0);
    assert_eq!(p["decided_by"], "seed");
    assert_eq!(p["winner"], teams[0].0.as_str());
    assert!(w.lease(&h, 16, 0).await.is_empty());
}
