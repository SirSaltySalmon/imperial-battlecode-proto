//! `GET /v1/matches/{id}/replay?view=public|team|full`: who may read which team's private data.

mod common;

use axum::http::StatusCode;
use common::{Harness, MockWorker, ADMIN};
use ibc_coordinator::util::sha256_hex;
use ibc_engine::abi::{Host, A_MOVE, DEBUG_LINE, MEM_TEAM};
use ibc_engine::{Map, Match, Ruleset};
use ibc_replay::{
    hash_hex, BotInfo, Header, HeaderRules, Keep, MatchResult, Record, Replay, ReplayWriter,
};
use serde_json::{json, Value};

const MAP: &str = include_str!("../../../maps/crossfire.ibcmap");

/// A short scripted match in which every unit writes memory, logs, uses fuel and draws.
fn real_replay(seed: u64) -> Replay {
    let rules = Ruleset::builtin();
    let map = Map::parse(MAP).unwrap();
    let bot = |n: &str| BotInfo {
        name: n.into(),
        sha256: sha256_hex(n.as_bytes()),
        fuel_multiplier: 1.0,
    };
    let header = Header {
        engine_version: ibc_engine::ENGINE_VERSION.into(),
        ruleset_hash: hash_hex(rules.hash()),
        map_hash: hash_hex(map.hash()),
        map_text: MAP.into(),
        seed,
        wasmtime_version: String::new(),
        abi_version: ibc_engine::abi::ABI_VERSION,
        bots: [bot("a"), bot("b")],
        result: MatchResult {
            winner: None,
            score: [0, 0],
            ticks: 0,
            finished: false,
        },
        team_size: 0,
        redacted: [false, false],
        rules: HeaderRules::of(&rules),
    };
    let mut m = Match::new(rules, map, seed);
    let mut w = ReplayWriter::new(Header {
        team_size: m.unit_count() / 2,
        ..header
    });
    for t in 0..20u32 {
        for unit in m.turn_order() {
            let team = m.unit_team(unit) as usize;
            m.act(unit, A_MOVE, (t * 1000 + unit * 77) as i32, 300, 0);
            m.mem_write(unit, MEM_TEAM, unit, &[t as u8]);
            m.debug_draw(unit, DEBUG_LINE, 0, 0, 100, 100, 0x00ff00);
            w.record(
                team,
                Record::Log {
                    unit,
                    text: format!("team {team} secret {t}\n").into_bytes(),
                },
            );
            w.record(
                team,
                Record::Fuel {
                    unit,
                    used: 500,
                    out_of_fuel: false,
                },
            );
        }
        w.inputs(m.take_inputs());
        w.draws(m.take_debug());
        m.step();
        w.end_tick(m.state_hashes());
    }
    w.finish(MatchResult {
        winner: m.winner(),
        score: m.score(),
        ticks: 20,
        finished: m.finished(),
    })
}

/// Complete a leased job with a real replay instead of the mock worker's fake one.
async fn complete_with(h: &Harness, w: &MockWorker, job: &Value, replay: &Replay) {
    let bytes = replay.encode();
    let sha = sha256_hex(&bytes);
    let (status, _) = h
        .raw("PUT", &format!("/v1/blobs/{sha}"), Some(&w.token), bytes)
        .await;
    assert!(status.is_success());
    let result = json!({"winner": 0, "outcome": "a", "score": [7, 3], "ticks": 20, "replay_sha256": sha,
                        "wall_ms": 5, "summary": {"outcome": "a", "replay_sha256": sha}});
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

async fn get(
    h: &Harness,
    match_id: &str,
    view: &str,
    token: Option<&str>,
) -> (StatusCode, Vec<u8>) {
    h.raw(
        "GET",
        &format!("/v1/matches/{match_id}/replay{view}"),
        token,
        vec![],
    )
    .await
}

#[tokio::test]
async fn replay_views_hide_the_other_teams_private_data() {
    let h = Harness::new(|_| {});
    let teams = h.league(&["alpha", "beta", "gamma"]).await;
    let (alpha, beta, gamma) = (&teams[0], &teams[1], &teams[2]);
    let batch = h
        .admin(
            "POST",
            "/v1/admin/ranked-batch",
            json!({"pairings": [["alpha", "beta"]], "games": 2}),
        )
        .await;
    let series_id = batch["series"][0].as_str().unwrap().to_string();
    let w = MockWorker::register(&h, "w", true).await;
    let stored: Vec<Replay> = (0..2).map(|i| real_replay(100 + i)).collect();
    for (job, replay) in w.lease(&h, 2, 0).await.iter().zip(&stored) {
        complete_with(&h, &w, job, replay).await;
    }

    let series = h
        .ok("GET", &format!("/v1/series/{series_id}"), None, Value::Null)
        .await;
    let ids: Vec<String> = series["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["match_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 2);
    for match_id in ids {
        let detail = h
            .ok("GET", &format!("/v1/matches/{match_id}"), None, Value::Null)
            .await;
        let full_sha = detail["replay_sha256"].as_str().unwrap().to_string();
        // Which side alpha played in this game (sides alternate between games).
        let alpha_side = if detail["bot_a"]["team_id"] == alpha.0.as_str() {
            0
        } else {
            1
        };
        assert_eq!(
            detail[["bot_a", "bot_b"][1 - alpha_side]]["team_id"],
            beta.0.as_str()
        );

        let (status, full) = get(&h, &match_id, "?view=full", Some(ADMIN)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            sha256_hex(&full),
            full_sha,
            "view=full is the stored record"
        );
        let full = Replay::decode(&full).unwrap();

        let (status, public) = get(&h, &match_id, "", None).await;
        assert_eq!(status, StatusCode::OK);
        let public = Replay::decode(&public).unwrap();
        assert_eq!(public, full.view(Keep::None));
        assert_eq!(
            get(&h, &match_id, "?view=public", None).await.1,
            public.encode()
        );
        ibc_replay::verify(&public).unwrap();

        for (team, side) in [(alpha, alpha_side), (beta, 1 - alpha_side)] {
            let (status, mine) = get(&h, &match_id, "?view=team", Some(&team.1)).await;
            assert_eq!(status, StatusCode::OK);
            let mine = Replay::decode(&mine).unwrap();
            assert_eq!(mine.header.redacted, [side == 1, side == 0], "{}", team.0);
            assert_eq!(mine, full.view(Keep::Team(side)));
            assert!(mine.ticks.iter().all(|t| t.private[1 - side].is_empty()));
            assert!(mine.ticks.iter().any(|t| !t.private[side].is_empty()));
            ibc_replay::verify(&mine).unwrap();
        }

        let status = |r: (StatusCode, Vec<u8>)| r.0;
        assert_eq!(
            status(get(&h, &match_id, "?view=team", Some(&gamma.1)).await),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(get(&h, &match_id, "?view=full", Some(&alpha.1)).await),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(get(&h, &match_id, "?view=team", Some(ADMIN)).await),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(get(&h, &match_id, "?view=team", None).await),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(get(&h, &match_id, "?view=full", None).await),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(get(&h, &match_id, "?view=everything", None).await),
            StatusCode::BAD_REQUEST
        );
    }
}
