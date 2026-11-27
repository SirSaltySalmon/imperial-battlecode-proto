//! The lease protocol against a mock worker: ordering, leases, retries, idempotency, verification.

mod common;

use axum::http::StatusCode;
use common::{fake_result, Harness, MockWorker, ADMIN};
use ibc_coordinator::util::sha256_hex;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn id(v: &Value) -> String {
    v["job_id"].as_str().unwrap().to_string()
}

async fn job_state(h: &Harness, job_id: &str) -> Value {
    h.admin("GET", &format!("/v1/admin/jobs/{job_id}"), Value::Null)
        .await
}

#[tokio::test]
async fn lease_hands_out_lowest_class_first_then_oldest() {
    let h = Harness::new(|_| {});
    h.set_patch().await;
    let mut queued = Vec::new();
    for (i, class) in ["bench", "unranked", "ranked", "tournament", "ranked"]
        .iter()
        .enumerate()
    {
        queued.push(id(&h.enqueue(class, i as u64).await));
        tokio::time::sleep(Duration::from_millis(3)).await; // distinct creation times
    }
    let w = MockWorker::register(&h, "w", false).await;
    let mut order = Vec::new();
    for _ in 0..5 {
        let job = w.lease_one(&h).await;
        assert_eq!(job["kind"], "match");
        assert_eq!(job["patch"]["wasmtime"], "51.0.0");
        assert_eq!(job["bot_a"]["fuel_mult"], 1.0);
        order.push(id(&job));
    }
    // tournament, the two ranked in age order, unranked, bench
    assert_eq!(
        order,
        [&queued[3], &queued[2], &queued[4], &queued[1], &queued[0]].map(String::clone)
    );
    assert!(w.lease(&h, 4, 0).await.is_empty());
}

#[tokio::test]
async fn aging_lets_an_old_bench_job_pass_newer_ranked_jobs_but_never_a_tournament() {
    let h = Harness::new(|c| c.aging_ms = 50);
    h.set_patch().await;
    let bench = id(&h.enqueue("bench", 1).await);
    tokio::time::sleep(Duration::from_millis(250)).await; // bench has aged as far as it can
    let ranked = id(&h.enqueue("ranked", 2).await);
    let tournament = id(&h.enqueue("tournament", 3).await);
    let w = MockWorker::register(&h, "w", false).await;
    assert_eq!(id(&w.lease_one(&h).await), tournament);
    assert_eq!(id(&w.lease_one(&h).await), bench);
    assert_eq!(id(&w.lease_one(&h).await), ranked);
}

#[tokio::test]
async fn seed_is_an_exact_u64() {
    let h = Harness::new(|_| {});
    h.set_patch().await;
    h.enqueue("bench", u64::MAX).await;
    let w = MockWorker::register(&h, "w", false).await;
    let (_, body) = h
        .raw(
            "POST",
            "/v1/jobs/lease",
            Some(&w.token),
            br#"{"max":1,"wait_ms":0}"#.to_vec(),
        )
        .await;
    assert!(String::from_utf8(body)
        .unwrap()
        .contains("\"seed\":18446744073709551615"));
}

#[tokio::test]
async fn heartbeat_extends_and_an_abandoned_lease_is_requeued() {
    let h = Harness::new(|c| c.lease_ms = 1000);
    h.set_patch().await;
    let job_id = id(&h.enqueue("ranked", 1).await);
    let (a, b) = (
        MockWorker::register(&h, "a", false).await,
        MockWorker::register(&h, "b", false).await,
    );

    let job = a.lease_one(&h).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let (status, beat) = a.heartbeat(&h, &job).await;
    assert_eq!(status, StatusCode::OK);
    assert!(beat["lease_expires_at"].as_str().unwrap().ends_with('Z'));
    // 1200 ms after the grant: only alive because of the heartbeat.
    tokio::time::sleep(Duration::from_millis(600)).await;
    h.app.sweep().unwrap();
    assert_eq!(job_state(&h, &job_id).await["state"], "leased");
    assert!(b.lease(&h, 1, 0).await.is_empty());

    // Worker A dies. The lease runs out and the sweeper returns the job to the queue.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(
        a.heartbeat(&h, &job).await.0,
        StatusCode::CONFLICT,
        "expired lease, even before the sweep"
    );
    h.app.sweep().unwrap();
    assert_eq!(job_state(&h, &job_id).await["state"], "queued");
    let again = b.lease_one(&h).await;
    assert_eq!(id(&again), job_id);
    assert_ne!(again["lease_token"], job["lease_token"]);

    // A comes back from the dead: its lease is gone.
    assert_eq!(a.heartbeat(&h, &job).await.0, StatusCode::CONFLICT);
    assert_eq!(a.fail(&h, &job, true).await.0, StatusCode::CONFLICT);
    let (status, err) = a.complete(&h, &job).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(err["error"], "lease_lost");
    assert_eq!(b.complete(&h, &again).await.0, StatusCode::OK);
    let done = job_state(&h, &job_id).await;
    assert_eq!(
        (done["state"].as_str(), done["attempts"].as_i64()),
        (Some("done"), Some(2))
    );
    assert_eq!(h.metric("ibc_leases_expired_total").await, 1.0);
}

#[tokio::test]
async fn three_attempts_then_dead_letter() {
    let h = Harness::new(|c| c.lease_ms = 600);
    h.set_patch().await;
    let w = MockWorker::register(&h, "w", false).await;

    // Three retryable failures.
    let retried = id(&h.enqueue("ranked", 1).await);
    for attempt in 1..=3 {
        let job = w.lease_one(&h).await;
        assert_eq!(w.fail(&h, &job, true).await.0, StatusCode::OK);
        assert_eq!(
            w.fail(&h, &job, true).await.0,
            StatusCode::OK,
            "fail is idempotent for the same lease token"
        );
        let state = job_state(&h, &retried).await;
        assert_eq!(state["attempts"], attempt);
        assert_eq!(state["state"], if attempt < 3 { "queued" } else { "dead" });
    }
    assert!(
        w.lease(&h, 1, 0).await.is_empty(),
        "a dead job is never retried silently"
    );

    // One non-retryable failure.
    let bad_input = id(&h.enqueue("ranked", 2).await);
    let job = w.lease_one(&h).await;
    assert_eq!(w.fail(&h, &job, false).await.0, StatusCode::OK);
    assert_eq!(job_state(&h, &bad_input).await["state"], "dead");

    // Three expired leases.
    let abandoned = id(&h.enqueue("ranked", 3).await);
    for attempt in 1..=3 {
        w.lease_one(&h).await;
        tokio::time::sleep(Duration::from_millis(700)).await;
        h.app.sweep().unwrap();
        assert_eq!(
            job_state(&h, &abandoned).await["state"],
            if attempt < 3 { "queued" } else { "dead" }
        );
    }

    let dead = h.admin("GET", "/v1/admin/dead", Value::Null).await;
    let dead: HashSet<String> = dead["jobs"].as_array().unwrap().iter().map(id).collect();
    assert_eq!(dead, HashSet::from([retried.clone(), bad_input, abandoned]));
    assert_eq!(h.metric("ibc_jobs_dead").await, 3.0);

    // A human can give a dead job another three attempts.
    h.admin(
        "POST",
        &format!("/v1/admin/jobs/{retried}/requeue"),
        Value::Null,
    )
    .await;
    let job = w.lease_one(&h).await;
    assert_eq!(id(&job), retried);
    assert_eq!(w.complete(&h, &job).await.0, StatusCode::OK);
}

#[tokio::test]
async fn a_failed_job_is_not_handed_straight_back_to_the_same_worker() {
    let h = Harness::new(|c| c.retry_grace_ms = 60_000);
    h.set_patch().await;
    h.enqueue("ranked", 1).await;
    let (old, new) = (
        MockWorker::register(&h, "old-patch", false).await,
        MockWorker::register(&h, "new-patch", false).await,
    );
    let job = old.lease_one(&h).await;
    assert_eq!(old.fail(&h, &job, true).await.0, StatusCode::OK);
    assert!(old.lease(&h, 1, 0).await.is_empty());
    assert_eq!(id(&new.lease_one(&h).await), id(&job));
}

#[tokio::test]
async fn enqueue_is_idempotent_and_a_done_job_is_a_cache_hit() {
    let h = Harness::new(|_| {});
    h.set_patch().await;
    let first = h.enqueue("bench", 7).await;
    assert_eq!(
        (first["created"].as_bool(), first["cached"].as_bool()),
        (Some(true), Some(false))
    );
    let second = h.enqueue("ranked", 7).await;
    assert_eq!(id(&second), id(&first));
    assert_eq!(
        (second["created"].as_bool(), second["cached"].as_bool()),
        (Some(false), Some(false))
    );
    assert_eq!(
        job_state(&h, &id(&first)).await["class"],
        "ranked",
        "the queued job took the higher priority"
    );
    assert_eq!(h.metric("ibc_queue_depth{class=\"ranked\"}").await, 1.0);

    let w = MockWorker::register(&h, "w", false).await;
    let job = w.lease_one(&h).await;
    assert_eq!(w.complete(&h, &job).await.0, StatusCode::OK);

    let third = h.enqueue("bench", 7).await;
    assert_eq!(id(&third), id(&first));
    assert_eq!(
        (third["created"].as_bool(), third["cached"].as_bool()),
        (Some(false), Some(true))
    );
    assert_eq!(third["result"], fake_result(&job, false).1);
    assert!(
        w.lease(&h, 1, 0).await.is_empty(),
        "a cache hit creates no work"
    );
    assert_eq!(h.metric("ibc_cache_hits_total").await, 1.0);
    assert_eq!(h.metric("ibc_jobs_per_minute").await, 1.0);
    // The other side order is a different match.
    assert_ne!(id(&h.enqueue("bench", 8).await), id(&first));
}

#[tokio::test]
async fn complete_checks_the_lease_token_the_result_and_the_replay_blob() {
    let h = Harness::new(|_| {});
    h.set_patch().await;
    let job_id = id(&h.enqueue("ranked", 1).await);
    let w = MockWorker::register(&h, "w", false).await;
    let job = w.lease_one(&h).await;
    let path = format!("/v1/jobs/{job_id}/complete");
    let (replay, result) = fake_result(&job, false);
    let sha = result["replay_sha256"].as_str().unwrap();
    let good = json!({"lease_token": job["lease_token"], "result": result});

    // No replay blob yet.
    let (status, err) = h.call("POST", &path, Some(&w.token), good.clone()).await;
    assert_eq!(
        (status, err["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("replay_missing"))
    );
    // A blob that does not hash to its name is refused.
    let (status, err) = h
        .call(
            "PUT",
            &format!("/v1/blobs/{sha}"),
            Some(&w.token),
            json!("not the replay"),
        )
        .await;
    assert_eq!(
        (status, err["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("hash_mismatch"))
    );
    assert_eq!(
        h.raw(
            "PUT",
            &format!("/v1/blobs/{sha}"),
            Some(&w.token),
            replay.clone()
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(
        h.raw(
            "PUT",
            &format!("/v1/blobs/{sha}"),
            Some(&w.token),
            replay.clone()
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        h.raw("GET", &format!("/v1/blobs/{sha}"), Some(ADMIN), vec![])
            .await,
        (StatusCode::OK, replay)
    );

    // Wrong lease token.
    let (status, err) = h
        .call(
            "POST",
            &path,
            Some(&w.token),
            json!({"lease_token": "nope", "result": result}),
        )
        .await;
    assert_eq!(
        (status, err["error"].as_str()),
        (StatusCode::CONFLICT, Some("lease_lost"))
    );
    // The right token presented by another worker.
    let other = MockWorker::register(&h, "other", false).await;
    assert_eq!(
        h.call("POST", &path, Some(&other.token), good.clone())
            .await
            .0,
        StatusCode::CONFLICT
    );
    // Malformed bodies and results.
    assert_eq!(
        h.raw("POST", &path, Some(&w.token), b"{not json".to_vec())
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let mut bad = good.clone();
    bad["result"]["winner"] = json!(2);
    assert_eq!(
        h.call("POST", &path, Some(&w.token), bad).await.0,
        StatusCode::BAD_REQUEST
    );
    // No token, wrong token, unknown job.
    assert_eq!(
        h.call("POST", &path, None, good.clone()).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        h.call("POST", &path, Some("ibw_bogus"), good.clone())
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        h.call(
            "POST",
            "/v1/jobs/nope/complete",
            Some(&w.token),
            good.clone()
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        job_state(&h, &job_id).await["state"],
        "leased",
        "nothing above touched the job"
    );

    // The real thing, twice: complete is idempotent for the same lease token.
    assert_eq!(
        h.call("POST", &path, Some(&w.token), good.clone()).await,
        (StatusCode::OK, json!({}))
    );
    assert_eq!(
        h.call("POST", &path, Some(&w.token), good).await,
        (StatusCode::OK, json!({}))
    );
    assert_eq!(h.metric("ibc_jobs_completed_total").await, 1.0);
    // A different result for a finished job with some other token is a stale lease.
    let (_, other_result) = fake_result(&job, true);
    h.blob(&fake_result(&job, true).0).await;
    let (status, _) = h
        .call(
            "POST",
            &path,
            Some(&w.token),
            json!({"lease_token": "stale", "result": other_result}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(job_state(&h, &job_id).await["result"], result);
}

async fn get_blob(h: &Harness, token: &str, sha: &str) -> StatusCode {
    let path = format!("/v1/blobs/{sha}");
    h.raw("GET", &path, Some(token), vec![]).await.0
}

/// The status and the error code of a blob upload.
async fn put_blob(h: &Harness, token: &str, bytes: &[u8]) -> (StatusCode, Option<String>) {
    let path = format!("/v1/blobs/{}", sha256_hex(bytes));
    let (status, body) = h.raw("PUT", &path, Some(token), bytes.to_vec()).await;
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, body["error"].as_str().map(str::to_owned))
}

/// The test patch, with `runner` as its `linux-x86_64` runner blob.
async fn set_patch_runner(h: &Harness, runner: &str) {
    h.admin(
        "POST",
        "/v1/admin/patch",
        json!({"engine": "0.1.0", "ruleset_hash": "00112233aabbccdd", "wasmtime": "51.0.0",
               "runner_blobs": {"linux-x86_64": runner}}),
    )
    .await;
}

#[tokio::test]
async fn a_worker_reads_only_the_blobs_of_its_leases_and_uploads_only_while_it_holds_one() {
    let h = Harness::new(|c| c.verify_fraction = 1.0);
    let runner = h.blob(b"pretend this is an ibc binary").await;
    set_patch_runner(&h, &runner).await;
    let job_id = id(&h.enqueue("ranked", 1).await);
    let mut named: Vec<String> = [b"bot a".as_slice(), b"bot b", b"a map"]
        .map(sha256_hex)
        .into();
    named.push(runner);
    let unrelated = h.blob(b"some other team's bot").await;
    let (w, idle) = (
        MockWorker::register(&h, "w", false).await,
        MockWorker::register(&h, "idle", false).await,
    );
    let lease_lost = (StatusCode::CONFLICT, Some("lease_lost".to_owned()));

    // Without a lease a worker reads nothing and writes nothing; the organisers are not restricted.
    for sha in &named {
        assert_eq!(get_blob(&h, &w.token, sha).await, StatusCode::FORBIDDEN);
        assert_eq!(get_blob(&h, ADMIN, sha).await, StatusCode::OK);
    }
    assert_eq!(put_blob(&h, &w.token, b"new").await, lease_lost);
    assert_eq!(put_blob(&h, ADMIN, b"new").await.0, StatusCode::CREATED);

    // With the job leased: its bots, map and patch runner, nothing else, and only for this worker.
    let job = w.lease_one(&h).await;
    for sha in &named {
        assert_eq!(get_blob(&h, &w.token, sha).await, StatusCode::OK);
        assert_eq!(get_blob(&h, &idle.token, sha).await, StatusCode::FORBIDDEN);
    }
    assert_eq!(
        get_blob(&h, &w.token, &unrelated).await,
        StatusCode::FORBIDDEN
    );
    // A blob that does not exist is refused like any other the lease does not name.
    assert_eq!(
        get_blob(&h, &w.token, &"0".repeat(64)).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(put_blob(&h, &idle.token, b"new").await, lease_lost);
    // The replay is uploaded under the lease. No lease names it, so it is not read back.
    let (replay, _) = fake_result(&job, false);
    assert_eq!(put_blob(&h, &w.token, &replay).await.0, StatusCode::CREATED);
    let replay_sha = sha256_hex(&replay);
    assert_eq!(
        get_blob(&h, &w.token, &replay_sha).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(w.complete(&h, &job).await.0, StatusCode::OK);

    // The lease is over, and so is the access. The trusted re-run reads the same blobs.
    assert_eq!(
        get_blob(&h, &w.token, &named[0]).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(put_blob(&h, &w.token, b"late").await, lease_lost);
    let trusted = MockWorker::register(&h, "trusted", true).await;
    assert_ne!(id(&trusted.lease_one(&h).await), job_id);
    for sha in &named {
        assert_eq!(get_blob(&h, &trusted.token, sha).await, StatusCode::OK);
    }

    // An expired lease that the sweeper has not reclaimed yet grants nothing.
    let expire = "UPDATE jobs SET lease_expires_at = 1 WHERE state = 'leased'";
    h.app.db.tx(|tx| Ok(tx.execute(expire, [])?)).unwrap();
    assert_eq!(
        get_blob(&h, &trusted.token, &named[0]).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(put_blob(&h, &trusted.token, b"late").await, lease_lost);
}

#[tokio::test]
async fn a_lease_keeps_the_runner_it_was_sent_when_the_organisers_replace_it() {
    let h = Harness::new(|_| {});
    let old = h.blob(b"pretend this is an ibc binary").await;
    set_patch_runner(&h, &old).await;
    h.enqueue("ranked", 1).await;
    h.enqueue("ranked", 2).await;
    let (a, b) = (
        MockWorker::register(&h, "a", false).await,
        MockWorker::register(&h, "b", false).await,
    );
    a.lease_one(&h).await;
    let new = h.blob(b"the same patch, rebuilt").await;
    set_patch_runner(&h, &new).await;
    assert_eq!(get_blob(&h, &a.token, &old).await, StatusCode::OK);
    assert_eq!(get_blob(&h, &a.token, &new).await, StatusCode::FORBIDDEN);
    let job = b.lease_one(&h).await;
    assert_eq!(job["runner_blobs"]["linux-x86_64"], new.as_str());
    assert_eq!(get_blob(&h, &b.token, &new).await, StatusCode::OK);
    assert_eq!(get_blob(&h, &b.token, &old).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_missing_blob_that_a_lease_names_is_404_so_the_job_fails() {
    let h = Harness::new(|_| {});
    h.set_patch().await;
    h.enqueue("ranked", 1).await;
    let w = MockWorker::register(&h, "w", false).await;
    w.lease_one(&h).await;
    let map = sha256_hex(b"a map");
    assert!(h.app.blobs.delete(&map).unwrap());
    assert_eq!(get_blob(&h, &w.token, &map).await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn registration_needs_the_shared_secret_and_trust_cannot_be_self_declared() {
    let h = Harness::new(|_| {});
    let (status, _) = h
        .call(
            "POST",
            "/v1/workers/register",
            None,
            json!({"registration_token": "wrong", "name": "x", "labels": [], "cores": 1}),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let out = h.ok("POST", "/v1/workers/register", None, json!({"registration_token": common::REG, "name": "liar", "labels": ["trusted"], "cores": 2})).await;
    let trusted = MockWorker::register(&h, "real", true).await;
    let workers = h.admin("GET", "/v1/admin/workers", Value::Null).await;
    let find = |id: &str| {
        workers["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["worker_id"] == id)
            .unwrap()
            .clone()
    };
    let liar = find(out["worker_id"].as_str().unwrap());
    assert_eq!(liar["trusted"], false);
    assert!(!liar["labels"]
        .as_array()
        .unwrap()
        .contains(&json!("trusted")));
    assert!(find(&trusted.id)["labels"]
        .as_array()
        .unwrap()
        .contains(&json!("trusted")));
    assert_eq!(h.metric("ibc_workers_live").await, 2.0);
    assert_eq!(h.metric("ibc_workers_cores").await, 6.0);
}

#[tokio::test]
async fn spot_verification_confirms_honest_workers_and_quarantines_a_cheat() {
    let h = Harness::new(|c| c.verify_fraction = 1.0);
    h.set_patch().await;
    let honest = MockWorker::register(&h, "honest", false).await;
    let mut cheat = MockWorker::register(&h, "cheat", false).await;
    cheat.cheat = true;
    let trusted = MockWorker::register(&h, "trusted", true).await;

    // An honest untrusted result is provisional until a trusted worker reproduces it.
    let honest_job = id(&h.enqueue("ranked", 1).await);
    let job = honest.lease_one(&h).await;
    assert_eq!(honest.complete(&h, &job).await.0, StatusCode::OK);
    assert_eq!(job_state(&h, &honest_job).await["verified"], false);
    assert_eq!(h.metric("ibc_verify_pending").await, 1.0);
    assert!(
        honest.lease(&h, 4, 0).await.is_empty(),
        "verify jobs go to trusted workers only"
    );
    let verify = trusted.lease_one(&h).await;
    assert_ne!(id(&verify), honest_job);
    assert_eq!(
        verify["kind"], "match",
        "on the wire a verification is an ordinary match"
    );
    assert_eq!(
        (&verify["seed"], &verify["bot_a"]),
        (&job["seed"], &job["bot_a"])
    );
    assert_eq!(trusted.complete(&h, &verify).await.0, StatusCode::OK);
    assert_eq!(job_state(&h, &honest_job).await["verified"], true);
    assert_eq!(h.metric("ibc_verify_confirmed_total").await, 1.0);

    // The cheat completes two jobs with tampered replays.
    let (cheated_1, cheated_2) = (
        id(&h.enqueue("ranked", 2).await),
        id(&h.enqueue("ranked", 3).await),
    );
    for job in cheat.lease(&h, 2, 0).await {
        assert_eq!(cheat.complete(&h, &job).await.0, StatusCode::OK);
    }
    let tampered = job_state(&h, &cheated_1).await["result"].clone();
    // The trusted worker re-runs the first one and disagrees.
    let verify = trusted.lease_one(&h).await;
    assert_eq!(
        job_state(&h, &id(&verify)).await["verifies"],
        cheated_1.as_str()
    );
    assert_eq!(trusted.complete(&h, &verify).await.0, StatusCode::OK);
    assert_eq!(h.metric("ibc_verify_mismatch_total").await, 1.0);

    // The trusted result wins...
    let fixed = job_state(&h, &cheated_1).await;
    assert_eq!(
        (fixed["state"].as_str(), fixed["verified"].as_bool()),
        (Some("done"), Some(true))
    );
    assert_ne!(fixed["result"], tampered);
    assert_eq!(fixed["result"], fake_result(&verify, false).1);
    assert_eq!(fixed["worker_id"], trusted.id.as_str());
    // ...the cheat is quarantined and gets nothing any more...
    let workers = h.admin("GET", "/v1/admin/workers", Value::Null).await;
    let row = workers["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["worker_id"] == cheat.id.as_str())
        .unwrap()
        .clone();
    assert_eq!(row["quarantined"], true);
    assert!(row["quarantine_reason"]
        .as_str()
        .unwrap()
        .contains(&cheated_1));
    h.enqueue("ranked", 4).await;
    assert!(cheat.lease(&h, 4, 0).await.is_empty());
    assert_eq!(h.metric("ibc_workers_quarantined").await, 1.0);
    // ...and its other unverified result was thrown away and queued again, while results of other workers stay.
    let requeued = job_state(&h, &cheated_2).await;
    assert_eq!(
        (requeued["state"].as_str(), &requeued["result"]),
        (Some("queued"), &Value::Null)
    );
    assert_eq!(job_state(&h, &honest_job).await["state"], "done");

    // Everything left is finished by the honest fleet; the re-run is confirmed against its verification.
    while honest.drain(&h).await + trusted.drain(&h).await > 0 {}
    let rerun = job_state(&h, &cheated_2).await;
    assert_eq!(
        (rerun["state"].as_str(), rerun["verified"].as_bool()),
        (Some("done"), Some(true))
    );
    assert_eq!(h.metric("ibc_verify_pending").await, 0.0);
}

#[tokio::test]
async fn trusted_results_are_final_and_tournament_jobs_are_always_verified() {
    let h = Harness::new(|c| c.verify_fraction = 0.000_001);
    h.set_patch().await;
    let trusted = MockWorker::register(&h, "trusted", true).await;
    let ranked = id(&h.enqueue("ranked", 1).await);
    assert_eq!(trusted.drain(&h).await, 1);
    assert_eq!(job_state(&h, &ranked).await["verified"], true);
    assert_eq!(h.metric("ibc_verify_pending").await, 0.0);

    let untrusted = MockWorker::register(&h, "untrusted", false).await;
    h.enqueue("tournament", 2).await;
    assert_eq!(untrusted.drain(&h).await, 1);
    assert_eq!(h.metric("ibc_verify_pending").await, 1.0);
}

#[tokio::test]
async fn long_poll_returns_early_when_a_job_arrives_and_empty_after_the_wait() {
    let h = Arc::new(Harness::new(|c| c.max_wait_ms = 400));
    h.set_patch().await;
    let w = MockWorker::register(&h, "w", false).await;

    let start = Instant::now();
    assert!(w.lease(&h, 1, 60_000).await.is_empty());
    let waited = start.elapsed();
    assert!(
        waited >= Duration::from_millis(380) && waited < Duration::from_millis(2000),
        "capped at max_wait_ms: {waited:?}"
    );

    let h2 = h.clone();
    let poll = tokio::spawn(async move {
        let w = MockWorker::register(&h2, "poller", false).await;
        let start = Instant::now();
        (w.lease(&h2, 1, 400).await, start.elapsed())
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.enqueue("bench", 1).await;
    let (jobs, waited) = poll.await.unwrap();
    assert_eq!(jobs.len(), 1);
    assert!(
        waited < Duration::from_millis(350),
        "woken by the enqueue, not by the timeout: {waited:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn eight_workers_never_lease_the_same_job_twice() {
    const JOBS: u64 = 240;
    let h = Arc::new(Harness::new(|_| {}));
    h.set_patch().await;
    let mut all = HashSet::new();
    for seed in 0..JOBS {
        all.insert(id(&h
            .enqueue(
                ["tournament", "ranked", "unranked", "bench"][(seed % 4) as usize],
                seed,
            )
            .await));
    }
    let mut tasks = Vec::new();
    for n in 0..8u32 {
        let h = h.clone();
        tasks.push(tokio::spawn(async move {
            let w = MockWorker::register(&h, &format!("w{n}"), false).await;
            let mut leased = Vec::new();
            loop {
                let jobs = w.lease(&h, 1 + n % 4, 0).await;
                if jobs.is_empty() {
                    return leased;
                }
                for job in jobs {
                    assert_eq!(w.complete(&h, &job).await.0, StatusCode::OK);
                    leased.push((id(&job), job["lease_token"].as_str().unwrap().to_string()));
                }
            }
        }));
    }
    let mut seen = HashSet::new();
    let mut tokens = HashSet::new();
    for task in tasks {
        for (job, token) in task.await.unwrap() {
            assert!(seen.insert(job.clone()), "job {job} was leased twice");
            assert!(tokens.insert(token));
        }
    }
    assert_eq!(seen, all);
    assert_eq!(h.metric("ibc_jobs_completed_total").await, JOBS as f64);
    assert_eq!(h.metric("ibc_jobs_leased").await, 0.0);
    let (status, _) = h
        .call("GET", "/v1/admin/dead", Some(ADMIN), Value::Null)
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_worker_quarantined_while_it_long_polls_gets_nothing() {
    let h = Arc::new(Harness::new(|c| c.max_wait_ms = 500));
    h.set_patch().await;
    let w = MockWorker::register(&h, "w", false).await;
    let (h2, token) = (h.clone(), w.token.clone());
    let poll = tokio::spawn(async move {
        h2.ok(
            "POST",
            "/v1/jobs/lease",
            Some(&token),
            json!({"max": 1, "wait_ms": 500}),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.admin(
        "POST",
        &format!("/v1/admin/workers/{}/quarantine", w.id),
        json!({"quarantined": true}),
    )
    .await;
    h.enqueue("ranked", 1).await;
    assert_eq!(poll.await.unwrap()["jobs"], json!([]));
    h.admin(
        "POST",
        &format!("/v1/admin/workers/{}/quarantine", w.id),
        json!({"quarantined": false}),
    )
    .await;
    assert_eq!(w.lease(&h, 1, 0).await.len(), 1);
}
