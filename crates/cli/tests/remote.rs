//! The platform commands of `ibc` against an in-process mock server: device login, token login, 429
//! handling, endpoint fallbacks, exit codes and the `--json` shapes agents rely on.

mod mock;

use mock::{Mock, Request, Response};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A fresh scratch directory under the target dir.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("remote-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run `ibc` with its config in `cfg` and no server or token from the environment.
fn ibc(cfg: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ibc"))
        .args(args)
        .env("IBC_CONFIG_DIR", cfg)
        .env_remove("IBC_SERVER")
        .env_remove("IBC_TOKEN")
        // The daily version check would ask the stored (mock) server first, and write the user's cache.
        .env("IBC_NO_UPDATE_CHECK", "1")
        .current_dir(cfg)
        .output()
        .unwrap()
}

fn stdout_json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap()
}

fn save_creds(cfg: &Path, server: &str, token: &str) {
    std::fs::write(
        cfg.join("credentials.json"),
        json!({"server": server, "token": token}).to_string(),
    )
    .unwrap();
}

fn bearer(r: &Request) -> Option<&str> {
    r.header("authorization")
        .and_then(|h| h.strip_prefix("Bearer "))
}

#[test]
fn device_login_polls_until_approved_and_saves_the_token() {
    let polls = Arc::new(AtomicUsize::new(0));
    let p = polls.clone();
    let mock = Mock::start(move |r| match r.path() {
        "/v1/auth/device" => Response::json(
            200,
            json!({"device_code": "dev-1", "user_code": "WXYZ-1234", "verify_url": "http://platform/device",
                   "interval": 0, "expires_in": 60}),
        ),
        "/v1/auth/device/token" if p.fetch_add(1, Ordering::SeqCst) < 2 => Response::json(
            428,
            json!({"error": "authorization_pending", "message": "not approved yet"}),
        ),
        "/v1/auth/device/token" => Response::json(200, json!({"token": "ibu_fresh"})),
        "/v1/me" => Response::json(
            200,
            json!({"user": {"id": "u1", "email": "a@b.c", "display_name": "Ada", "is_admin": false},
                   "team": {"id": "t1", "name": "alpha", "join_code": "JOIN"}}),
        ),
        _ => Response::bytes(404, b""),
    });
    let cfg = scratch("device");
    let out = ibc(
        &cfg,
        &["login", "--server", &format!("{}/v1/", mock.url), "--json"],
    );
    assert_eq!(code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stderr).contains("WXYZ-1234"));
    let v = stdout_json(&out);
    assert_eq!(v["token_kind"], "user");
    assert_eq!(v["user"]["display_name"], "Ada");
    assert_eq!(v["team"]["id"], "t1");
    assert_eq!(v["server"], mock.url.as_str());
    assert_eq!(polls.load(Ordering::SeqCst), 3);
    let saved: Value =
        serde_json::from_slice(&std::fs::read(cfg.join("credentials.json")).unwrap()).unwrap();
    assert_eq!(saved, json!({"server": mock.url, "token": "ibu_fresh"}));
    let reqs = mock.requests();
    let token_poll = reqs
        .iter()
        .find(|r| r.path() == "/v1/auth/device/token")
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&token_poll.body).unwrap(),
        json!({"device_code": "dev-1"})
    );
    assert_eq!(
        bearer(reqs.iter().find(|r| r.path() == "/v1/me").unwrap()),
        Some("ibu_fresh")
    );

    // whoami and logout use what was saved.
    let who = stdout_json(&ibc(&cfg, &["whoami", "--json"]));
    assert_eq!(who["team"]["name"], "alpha");
    let out = ibc(&cfg, &["logout", "--json"]);
    assert_eq!(stdout_json(&out)["logged_out"], true);
    assert!(mock
        .requests()
        .iter()
        .any(|r| r.path() == "/v1/auth/logout" && bearer(r) == Some("ibu_fresh")));
    assert!(!cfg.join("credentials.json").exists());
    assert_eq!(code(&ibc(&cfg, &["whoami"])), 3);
}

#[test]
fn an_expired_device_code_is_an_auth_failure() {
    let mock = Mock::start(|r| match r.path() {
        "/v1/auth/device" => Response::json(
            200,
            json!({"device_code": "d", "user_code": "U", "verify_url": "v", "interval": 0, "expires_in": 60}),
        ),
        _ => Response::json(
            400,
            json!({"error": "expired_token", "message": "the code expired"}),
        ),
    });
    let cfg = scratch("expired");
    let out = ibc(&cfg, &["login", "--server", &mock.url, "--json"]);
    assert_eq!(code(&out), 3);
    assert_eq!(stdout_json(&out)["error"], "expired_token");
    assert!(!cfg.join("credentials.json").exists());
}

#[test]
fn a_pasted_token_is_checked_before_it_is_saved() {
    let mock = Mock::start(|r| match bearer(r) {
        Some("ibt_good") => Response::json(
            200,
            json!({"team_id": "t9", "name": "beta", "live_submission": null}),
        ),
        _ => Response::json(
            401,
            json!({"error": "unauthorized", "message": "missing or invalid bearer token"}),
        ),
    });
    let cfg = scratch("token");
    let out = ibc(
        &cfg,
        &["login", "--server", &mock.url, "--token", "ibt_bad"],
    );
    assert_eq!(code(&out), 3);
    assert!(String::from_utf8_lossy(&out.stderr).contains("unauthorized"));
    assert!(!cfg.join("credentials.json").exists());

    assert_eq!(
        code(&ibc(
            &cfg,
            &["login", "--server", &mock.url, "--token", "nonsense"]
        )),
        2
    );

    let out = ibc(
        &cfg,
        &[
            "login", "--server", &mock.url, "--token", "ibt_good", "--json",
        ],
    );
    assert_eq!(code(&out), 0);
    let v = stdout_json(&out);
    assert_eq!(
        (v["token_kind"].as_str(), v["team"]["id"].as_str()),
        (Some("team"), Some("t9"))
    );
    // A team token is checked with `GET team`, never `GET me`.
    assert!(mock.requests().iter().all(|r| r.path() == "/v1/team"));
}

#[test]
fn rate_limits_show_the_wait_and_wait_retries_once() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let mock = Mock::start(move |_| {
        if c.fetch_add(1, Ordering::SeqCst) < 3 {
            Response::json(
                429,
                json!({"error": "rate_limited", "message": "slow down", "retry_after": 7}),
            )
            .with_header("Retry-After", "1")
        } else {
            Response::json(
                200,
                json!({"generated_at": "now", "teams": [{"rank": 1, "team_id": "t1", "name": "alpha", "rating": 1500}]}),
            )
        }
    });
    let cfg = scratch("ratelimit");
    let out = ibc(&cfg, &["ladder", "--server", &mock.url, "--json"]);
    assert_eq!(code(&out), 1);
    let e = stdout_json(&out);
    assert_eq!(
        e,
        json!({"error": "rate_limited", "message": "slow down", "status": 429, "retry_after": 1})
    );

    let out = ibc(&cfg, &["ladder", "--server", &mock.url]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("rate limited") && err.contains("retry after 1 s"),
        "{err}"
    );

    let out = ibc(&cfg, &["ladder", "--server", &mock.url, "--wait", "--json"]);
    assert_eq!(code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(stdout_json(&out)["teams"][0]["team_name"], "alpha");
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[test]
fn missing_server_or_token_exits_3_and_stored_tokens_stay_with_their_server() {
    let mock = Mock::start(|_| Response::json(200, json!({"submissions": []})));
    let cfg = scratch("auth");
    assert_eq!(code(&ibc(&cfg, &["ladder"])), 3);
    assert_eq!(code(&ibc(&cfg, &["submissions", "--server", &mock.url])), 3);
    save_creds(&cfg, "http://elsewhere.invalid", "ibt_secret");
    // The stored token belongs to another server: it is not sent here.
    assert_eq!(code(&ibc(&cfg, &["submissions", "--server", &mock.url])), 3);
    assert!(mock.requests().is_empty());
}

#[test]
fn json_shapes_of_the_read_commands() {
    let mock = Mock::start(|r| match r.path() {
        "/v1/ladder" => Response::json(
            200,
            json!([
            {"rank": 1, "team_id": "t1", "team_name": "alpha", "rating": 1612, "tier": null, "provisional": false, "games": 30},
            {"rank": 2, "team_id": "t2", "team_name": "beta", "rating": null, "tier": null, "provisional": true, "games": 0}]),
        ),
        "/v1/team" => Response::json(
            200,
            json!({"team_id": "t1", "name": "alpha", "live_submission": "s1"}),
        ),
        "/v1/matches" => Response::json(
            200,
            json!({"next": null, "matches": [
            {"match_id": "m1", "kind": "ranked", "map": "crossfire", "bot_a": {"team_id": "t1", "name": "alpha"},
             "bot_b": {"team_id": "t2", "name": "beta"}, "state": "done", "outcome": "a", "score": [7, 3]},
            {"match_id": "m2", "kind": "unranked", "map": "crossfire", "bot_a": {"team_id": "t1", "name": "alpha"},
             "bot_b": {"team_id": "t2", "name": "beta"}, "state": "queued", "outcome": null}]}),
        ),
        "/v1/submissions" => Response::json(
            200,
            json!({"submissions": [{"submission_id": "s1", "live": true, "status": "validated"}]}),
        ),
        "/v1/submissions/s0/live" => Response::json(
            200,
            json!({"team_id": "t1", "live_submission": "s0", "changed": true}),
        ),
        _ => Response::bytes(404, b""),
    });
    let cfg = scratch("shapes");
    save_creds(&cfg, &mock.url, "ibt_t1");

    let v = stdout_json(&ibc(&cfg, &["ladder", "--top", "1", "--json"]));
    assert_eq!(v["teams"].as_array().unwrap().len(), 1);
    assert_eq!(v["teams"][0]["team_name"], "alpha");

    let v = stdout_json(&ibc(
        &cfg,
        &[
            "matches", "--mine", "--kind", "ranked", "--limit", "5", "--json",
        ],
    ));
    let ids: Vec<&str> = v["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["match_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["m1"]);
    let q = mock
        .requests()
        .into_iter()
        .find(|r| r.path() == "/v1/matches")
        .unwrap()
        .target;
    assert_eq!(q, "/v1/matches?team=t1&kind=ranked&limit=5");

    let v = stdout_json(&ibc(&cfg, &["submissions", "--json"]));
    assert_eq!(v["submissions"][0]["submission_id"], "s1");
    let v = stdout_json(&ibc(&cfg, &["submissions", "--live", "s0", "--json"]));
    assert_eq!(v["live_submission"], "s0");
    let text = String::from_utf8_lossy(&ibc(&cfg, &["matches"]).stdout).into_owned();
    assert!(
        text.contains("m1") && text.contains("alpha vs beta"),
        "{text}"
    );
}

#[test]
fn scrim_requests_an_unranked_series_from_the_quota() {
    let mock = Mock::start(|r| match r.path() {
        "/v1/scrims" => Response::json(
            201,
            json!({"series_id": "sr1", "games": 3, "state": "running",
            "team_a": {"name": "alpha"}, "team_b": {"name": "beta"}}),
        ),
        _ => Response::bytes(404, b""),
    });
    let cfg = scratch("scrim");
    save_creds(&cfg, &mock.url, "ibt_t1");
    let out = ibc(
        &cfg,
        &["scrim", "beta", "--games", "3", "--maps", "a,b", "--json"],
    );
    assert_eq!(code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(stdout_json(&out)["series_id"], "sr1");
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(
        serde_json::from_slice::<Value>(&reqs[0].body).unwrap(),
        json!({"opponent_team_id": "beta", "games": 3, "maps": ["a", "b"]})
    );
}

#[test]
fn maps_pull_writes_the_pool_and_keeps_custom_maps() {
    let mock = Mock::start(|r| match r.path() {
        "/v1/maps" => Response::json(
            200,
            json!({"maps": [
            {"name": "alpha", "sha256": sha("A"), "in_pool": true},
            {"name": "retired", "sha256": sha("R"), "in_pool": false},
            {"name": "bad", "sha256": sha("expected"), "in_pool": true}]}),
        ),
        "/v1/maps/alpha" => Response::bytes(200, b"A"),
        "/v1/maps/retired" => Response::bytes(200, b"R"),
        "/v1/maps/bad" => Response::bytes(200, b"tampered"),
        _ => Response::bytes(404, b""),
    });
    let cfg = scratch("maps");
    let dir = cfg.join("maps");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("mine.ibcmap"), "custom").unwrap();
    let out = ibc(
        &cfg,
        &[
            "maps", "pull", "--server", &mock.url, "--dir", "maps", "--json",
        ],
    );
    assert_eq!(code(&out), 1, "the tampered map fails the command");
    let v = stdout_json(&out);
    let status: Vec<(&str, &str)> = v["maps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m["name"].as_str().unwrap(), m["status"].as_str().unwrap()))
        .collect();
    assert_eq!(status, [("alpha", "new"), ("bad", "failed")]);
    assert_eq!(v["maps"][1]["error"], "hash_mismatch");
    assert_eq!(
        std::fs::read_to_string(dir.join("alpha.ibcmap")).unwrap(),
        "A"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("mine.ibcmap")).unwrap(),
        "custom"
    );
    assert!(!dir.join("bad.ibcmap").exists() && !dir.join("retired.ibcmap").exists());

    let out = ibc(
        &cfg,
        &[
            "maps", "pull", "--server", &mock.url, "--dir", "maps", "--all", "--json",
        ],
    );
    let v = stdout_json(&out);
    assert_eq!(v["maps"][0]["status"], "unchanged");
    assert_eq!(v["maps"][1]["status"], "new");
}

fn sha(text: &str) -> String {
    sha_bytes(text.as_bytes())
}

#[test]
fn replay_download_saves_and_reads_the_replay() {
    let replay = std::fs::read(repo().join("fixtures/sample.ibcr")).unwrap();
    let body = replay.clone();
    let mock = Mock::start(move |r| match r.path() {
        "/v1/matches/m7/replay" => Response::bytes(200, &body),
        _ => Response::json(404, json!({"error": "not_found", "message": "no match"})),
    });
    let cfg = scratch("replay");
    let out = ibc(
        &cfg,
        &[
            "replay", "download", "m7", "--server", &mock.url, "--view", "public", "--json",
        ],
    );
    assert_eq!(code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let v = stdout_json(&out);
    assert_eq!(v["bytes"], replay.len());
    assert!(
        v["result"]["ticks"].as_u64().is_some() && v.get("decode_error").is_none(),
        "{v}"
    );
    assert_eq!(
        std::fs::read(cfg.join("out/replays/m7.ibcr")).unwrap(),
        replay
    );
    assert_eq!(
        mock.requests()[0].target,
        "/v1/matches/m7/replay?view=public"
    );

    let out = ibc(
        &cfg,
        &[
            "replay", "download", "nope", "--server", &mock.url, "--json",
        ],
    );
    assert_eq!(code(&out), 1);
    assert_eq!(stdout_json(&out)["error"], "not_found");

    // An id that is not a safe file name needs -o, and nothing is fetched.
    let before = mock.requests().len();
    let out = ibc(&cfg, &["replay", "download", "../x", "--server", &mock.url]);
    assert_eq!(code(&out), 2);
    assert_eq!(mock.requests().len(), before);
}

#[test]
fn maps_pull_stops_once_on_a_server_without_map_text() {
    let mock = Mock::start(|r| match r.path() {
        "/v1/maps" => Response::json(
            200,
            json!({"maps": [{"name": "a", "sha256": sha("A"), "in_pool": true},
                            {"name": "b", "sha256": sha("B"), "in_pool": true}]}),
        ),
        _ => Response::bytes(404, b""),
    });
    let cfg = scratch("maps-old");
    let out = ibc(&cfg, &["maps", "pull", "--server", &mock.url, "--json"]);
    assert_eq!(code(&out), 1);
    assert_eq!(stdout_json(&out)["error"], "endpoint_missing");
    assert_eq!(mock.requests().len(), 2);
}

#[test]
fn submit_uploads_a_prebuilt_module() {
    let wasm = std::fs::read(repo().join("fixtures/golden/bots/idle.wasm")).unwrap();
    let mock = Mock::start(|r| {
        let hash = sha_bytes(&r.body);
        Response::json(
            201,
            json!({"submission_id": "s5", "sha256": hash, "status": "validated", "live": false}),
        )
    });
    let cfg = scratch("submit");
    save_creds(&cfg, &mock.url, "ibt_t1");
    let path = repo().join("fixtures/golden/bots/idle.wasm");
    let out = ibc(
        &cfg,
        &[
            "submit",
            path.to_str().unwrap(),
            "--note",
            "first try",
            "--json",
        ],
    );
    assert_eq!(code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let v = stdout_json(&out);
    assert_eq!(v["submission"]["submission_id"], "s5");
    assert_eq!(v["module"]["sha256"], sha_bytes(&wasm));
    assert_eq!(v["module"]["language"], "rust");
    let req = &mock.requests()[0];
    assert_eq!(
        req.target,
        "/v1/submissions?language=rust&live=false&note=first%20try"
    );
    assert_eq!((req.method.as_str(), &req.body), ("POST", &wasm));
    assert_eq!(bearer(req), Some("ibt_t1"));
}

fn sha_bytes(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(b)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
