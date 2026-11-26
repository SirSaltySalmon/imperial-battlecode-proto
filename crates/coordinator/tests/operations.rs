//! What the operators run around the server: the published distribution, backups and replay retention.

mod common;

use axum::http::StatusCode;
use common::{Harness, MockWorker};
use ibc_coordinator::maintenance::{self, BackupReport};
use ibc_coordinator::model::Class;
use ibc_coordinator::util::{now_ms, random_hex};
use serde_json::{json, Value};

fn scratch(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("ibc-ops-{tag}-{}", random_hex(6)))
}

#[tokio::test]
async fn the_distribution_is_served_from_its_file_or_explained_when_missing() {
    let h = Harness::new(|_| {});
    let (status, err) = h.call("GET", "/v1/distribution", None, Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        err["message"]
            .as_str()
            .unwrap()
            .contains("IBC_DISTRIBUTION"),
        "{err}"
    );

    let dir = scratch("dist");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("distribution.json");
    let doc = json!({"version": "0.2.0", "patch": {"engine": "0.2.0"}, "pip": "imperial-battlecode==0.2.0",
        "artifacts": [{"target": "x86_64-unknown-linux-musl", "file": "ibc-0.2.0-x86_64-unknown-linux-musl.tar.gz",
                       "url": "https://example.org/ibc-0.2.0-x86_64-unknown-linux-musl.tar.gz",
                       "sha256": "ab".repeat(32), "size": 10}]});
    std::fs::write(&file, doc.to_string()).unwrap();
    let path = file.clone();
    let h = Harness::new(move |c| c.distribution_file = Some(path));
    assert_eq!(
        h.ok("GET", "/v1/distribution", None, Value::Null).await,
        doc
    );

    // Publishing is replacing the file; a broken file is a server error, never a half-read answer.
    std::fs::write(&file, "{\"version\": \"0.3.0\"").unwrap();
    let (status, _) = h.call("GET", "/v1/distribution", None, Value::Null).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    std::fs::write(
        &file,
        json!({"version": "0.3.0", "artifacts": []}).to_string(),
    )
    .unwrap();
    let now = h.ok("GET", "/v1/distribution", None, Value::Null).await;
    assert_eq!(now["version"], "0.3.0");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// One unranked and one ranked series of two games between alpha and beta, all played.
async fn played_league(h: &Harness) -> (Value, Value) {
    let teams = h.league(&["alpha", "beta"]).await;
    let unranked = h
        .ok(
            "POST",
            "/v1/series",
            Some(&teams[0].1),
            json!({"opponent": "beta", "games": 2}),
        )
        .await;
    let batch = h
        .admin(
            "POST",
            "/v1/admin/ranked-batch",
            json!({"pairings": [["alpha", "beta"]], "games": 2}),
        )
        .await;
    let w = MockWorker::register(h, "w", true).await;
    for job in w.lease(h, 8, 0).await {
        let (status, out) = w.complete(h, &job).await;
        assert!(status.is_success(), "{out}");
    }
    h.app.sweep().unwrap();
    let path = format!("/v1/series/{}", batch["series"][0].as_str().unwrap());
    let ranked = h.ok("GET", &path, None, Value::Null).await;
    let path = format!("/v1/series/{}", unranked["series_id"].as_str().unwrap());
    let unranked = h.ok("GET", &path, None, Value::Null).await;
    assert_eq!(
        (ranked["state"].as_str(), unranked["state"].as_str()),
        (Some("done"), Some("done"))
    );
    (unranked, ranked)
}

async fn replay_status(h: &Harness, series: &Value) -> Vec<StatusCode> {
    let mut out = Vec::new();
    for m in series["matches"].as_array().unwrap() {
        let url = m["replay_url"].as_str().unwrap();
        out.push(h.raw("GET", url, None, Vec::new()).await.0);
    }
    out
}

#[tokio::test]
async fn backups_snapshot_the_database_and_copy_only_new_blobs() {
    let h = Harness::new(|_| {});
    played_league(&h).await;
    let dest = scratch("backup");
    let first = maintenance::backup(h.data_dir(), &dest).unwrap();
    assert!(
        first.database_bytes > 0 && first.blobs_copied >= 6,
        "{first:?}"
    );
    assert_eq!(first.blobs_present, 0);
    let again = maintenance::backup(h.data_dir(), &dest).unwrap();
    assert_eq!(
        again,
        BackupReport {
            database_bytes: again.database_bytes,
            blobs_copied: 0,
            blobs_present: first.blobs_copied,
            bytes_copied: 0,
        }
    );

    // The backup is a working data directory: open it and find the same ladder.
    let restored = ibc_coordinator::App::open(h.app.cfg.clone(), &dest).unwrap();
    let ladder = |app: &ibc_coordinator::App| {
        app.db
            .tx(|tx| ibc_coordinator::db::ratings::public_ladder(tx, app.rating.as_ref(), &app.cfg))
            .unwrap()
    };
    assert_eq!(ladder(&restored), ladder(&h.app));
    drop(restored);
    std::fs::remove_dir_all(&dest).unwrap();
}

#[tokio::test]
async fn pruning_deletes_old_replays_of_the_chosen_kinds_only() {
    let h = Harness::new(|_| {});
    let (unranked, ranked) = played_league(&h).await;
    // A stand-alone bench job: its result is a cache for re-submissions, so its replay is never pruned.
    h.enqueue("bench", 7).await;
    let w = MockWorker::register(&h, "bench-worker", true).await;
    let job = w.lease_one(&h).await;
    assert!(w.complete(&h, &job).await.0.is_success());
    let ok = vec![StatusCode::OK; 2];
    assert_eq!(replay_status(&h, &unranked).await, ok);

    // Nothing is older than a day yet.
    let day_ago = now_ms() - 86_400_000;
    let r = maintenance::prune_replays(h.data_dir(), &[Class::Unranked], day_ago, false).unwrap();
    assert_eq!((r.replays, r.deleted), (0, 0));

    let later = now_ms() + 1_000;
    let dry =
        maintenance::prune_replays(h.data_dir(), &[Class::Unranked, Class::Bench], later, true)
            .unwrap();
    assert_eq!((dry.replays, dry.deleted), (2, 2));
    assert_eq!(replay_status(&h, &unranked).await, ok);

    let r =
        maintenance::prune_replays(h.data_dir(), &[Class::Unranked, Class::Bench], later, false)
            .unwrap();
    assert_eq!(r, dry);
    let gone = vec![StatusCode::NOT_FOUND; 2];
    assert_eq!(replay_status(&h, &unranked).await, gone);
    assert_eq!(replay_status(&h, &ranked).await, ok);
    // Results stay: the series still shows who won.
    let path = format!("/v1/series/{}", unranked["series_id"].as_str().unwrap());
    let after = h.ok("GET", &path, None, Value::Null).await;
    assert_eq!(
        after["matches"][0]["outcome"],
        unranked["matches"][0]["outcome"]
    );

    // Running it again finds the same matches but nothing left to delete.
    let r = maintenance::prune_replays(h.data_dir(), &[Class::Unranked], later, false).unwrap();
    assert_eq!((r.replays, r.deleted), (2, 0));
}
