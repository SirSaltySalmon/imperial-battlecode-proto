//! What the operators run around the server: its environment, the published distribution, backups and
//! replay retention.

mod common;

use axum::http::StatusCode;
use common::{Harness, MockWorker, ADMIN};
use ibc_coordinator::maintenance::{self, BackupReport};
use ibc_coordinator::model::Class;
use ibc_coordinator::util::{now_ms, random_hex};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

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

    // Configured before the first release is put there: a 404 that names the file, served once it exists.
    let dir = scratch("dist");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("distribution.json");
    let path = file.clone();
    let h = Harness::new(move |c| c.distribution_file = Some(path));
    let (status, err) = h.call("GET", "/v1/distribution", None, Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let message = err["message"].as_str().unwrap();
    assert!(message.contains(&file.display().to_string()), "{err}");
    let doc = json!({"version": "0.2.0", "patch": {"engine": "0.2.0"}, "pip": "imperial-battlecode==0.2.0",
        "artifacts": [{"target": "x86_64-unknown-linux-musl", "file": "ibc-0.2.0-x86_64-unknown-linux-musl.tar.gz",
                       "url": "https://example.org/ibc-0.2.0-x86_64-unknown-linux-musl.tar.gz",
                       "sha256": "ab".repeat(32), "size": 10}]});
    std::fs::write(&file, doc.to_string()).unwrap();
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

/// A child process killed when dropped, so a failed assertion leaves no server behind.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One HTTP/1.1 request (`"GET /path"`) to `addr`: the whole response, status line first.
fn http(addr: &str, request: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(addr).unwrap();
    write!(
        stream,
        "{request} HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

/// `${VAR:-}` in a compose file sets a variable to the empty string. The real binary, with every variable
/// its `--help` lists set that way, starts with each flag at its default: no distribution, and untrusted
/// workers register with the admin token.
#[test]
fn the_coordinator_starts_with_every_variable_set_empty() {
    let exe = env!("CARGO_BIN_EXE_ibc-coordinator");
    let help = Command::new(exe).arg("--help").output().unwrap().stdout;
    let help = String::from_utf8(help).unwrap();
    // Every IBC_ name in the text: `[env: IBC_NAME=value]`, `[env: IBC_NAME]` when values are hidden.
    let vars: BTreeSet<&str> = help
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| word.len() > 4 && word.starts_with("IBC_"))
        .collect();
    assert!(
        vars.contains("IBC_DISTRIBUTION") && vars.contains("IBC_TRUST_PROXY"),
        "{help}"
    );
    let data = scratch("empty-env");
    let mut cmd = Command::new(exe);
    for var in &vars {
        cmd.env(var, "");
    }
    cmd.env("IBC_DATA", &data)
        .env("IBC_ADMIN_TOKEN", ADMIN)
        .env("IBC_LISTEN", "127.0.0.1:0")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut server = Server(cmd.spawn().unwrap());
    let mut stderr = BufReader::new(server.0.stderr.take().unwrap());
    let mut log = String::new();
    let addr = loop {
        let mut line = String::new();
        assert!(stderr.read_line(&mut line).unwrap() > 0, "exited: {log}");
        log.push_str(&line);
        if let Some((_, rest)) = line.split_once("listening on http://") {
            break rest.split_whitespace().next().unwrap().to_string();
        }
    };
    let distribution = http(&addr, "GET /v1/distribution", "");
    assert!(distribution.starts_with("HTTP/1.1 404"), "{distribution}");
    assert!(
        distribution.contains("publishes no distribution"),
        "{distribution}"
    );
    let register = json!({"registration_token": ADMIN, "name": "w"}).to_string();
    let register = http(&addr, "POST /v1/workers/register", &register);
    assert!(register.starts_with("HTTP/1.1 200"), "{register}");
    drop(server);
    std::fs::remove_dir_all(&data).unwrap();
}

/// One unranked and one ranked series of two games between alpha and beta, all played.
async fn played_league(h: &Harness) -> (Value, Value) {
    let teams = h.league(&["alpha", "beta"]).await;
    let unranked = h
        .ok(
            "POST",
            "/v1/scrims",
            Some(&teams[0].1),
            json!({"opponent_team_id": "beta", "games": 2}),
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
        // The stored record: the mock worker's replays are not real .ibcr files, so only view=full serves them.
        let url = format!("{}?view=full", m["replay_url"].as_str().unwrap());
        out.push(h.raw("GET", &url, Some(ADMIN), Vec::new()).await.0);
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
