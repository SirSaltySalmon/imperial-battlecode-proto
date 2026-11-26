//! A play session run in-process: endpoints called directly, the match on its own thread, and the replay
//! of the scripted player checked by re-simulation.

use super::server::{respond, Request, Response, Site};
use super::session::{Mode, Session};
use super::{prepare, Setup};
use ibc_abi::*;
use ibc_replay::Event;
use ibc_runner::{BotSpec, MatchOutput};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const MAP: &str = include_str!("../../../../maps/crossfire.ibcmap");

/// Ends its turn immediately, forever.
const IDLE: &str = r#"(module
  (import "ibc" "end_turn" (func $end_turn))
  (memory (export "memory") 1)
  (func (export "_start") (loop $l (call $end_turn) (br $l))))"#;

fn idle(name: &str) -> BotSpec {
    BotSpec { name: name.into(), wasm: wat::parse_str(IDLE).unwrap(), fuel_multiplier: 1.0 }
}

struct Running {
    site: Site,
    base: PathBuf,
    game: JoinHandle<MatchOutput>,
}

fn start(name: &str, mode: Mode, team: i32, allies: bool) -> Running {
    let base = std::env::temp_dir().join(format!("ibc-play-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("dist")).unwrap();
    std::fs::write(base.join("dist").join("play.html"), "<!doctype html>play").unwrap();
    std::fs::write(base.join("secret.txt"), "secret").unwrap();
    let (cfg, session) = prepare(Setup {
        map_text: MAP.into(),
        seed: 5,
        mode,
        team,
        slot: 1,
        opponent: idle("opponent"),
        allies: allies.then(|| idle("mates")),
    })
    .unwrap();
    let s = Arc::clone(&session);
    let game = std::thread::spawn(move || ibc_runner::run_driven(cfg, &mut s.driver()).unwrap());
    let site = Site { root: base.join("dist").canonicalize().unwrap(), session };
    Running { site, base, game }
}

impl Running {
    fn call(&self, method: &str, path: &str, body: &str) -> Response {
        let req = Request {
            method: method.into(),
            path: path.into(),
            host: Some("127.0.0.1:18019".into()),
            play_header: true,
            body: body.as_bytes().to_vec(),
        };
        respond(&self.site, &req)
    }

    fn json(&self, method: &str, path: &str, body: &str) -> Value {
        let r = self.call(method, path, body);
        assert_eq!(r.status, "200 OK", "{method} {path}: {}", String::from_utf8_lossy(&r.body));
        serde_json::from_slice(&r.body).unwrap()
    }

    fn session(&self) -> &Session {
        &self.site.session
    }

    fn quit(self) -> MatchOutput {
        self.json("POST", "/quit", "");
        let out = self.game.join().unwrap();
        std::fs::remove_dir_all(&self.base).unwrap();
        out
    }
}

/// The acts recorded in the replay for `unit`, by kind.
fn recorded(out: &MatchOutput, unit: u32) -> Vec<u32> {
    out.replay
        .ticks
        .iter()
        .flat_map(|t| &t.events)
        .filter_map(|e| match e {
            Event::Input(ibc_engine::Input::Act { unit: u, kind, .. }) if *u == unit => Some(*kind),
            _ => None,
        })
        .collect()
}

#[test]
fn a_scripted_turn_based_session_replays() {
    let p = start("turn", Mode::Turn, 1, false);
    let setup = p.json("GET", "/setup", "");
    let me = setup["unit"].as_u64().unwrap() as u32;
    assert_eq!((setup["team"].clone(), setup["mode"].clone()), (json!(1), json!("turn")));
    assert_eq!(me, setup["core"]["team_size"].as_u64().unwrap() as u32 + 1, "slot 1 of team B");
    assert!(setup["kits"].as_array().unwrap().len() > 1 && setup["map_text"].is_string());

    let before = p.json("GET", "/state", "");
    assert_eq!((before["tick"].clone(), before["view"]["match"]["phase"].clone()), (json!(0), json!(PHASE_DRAFT)));
    p.json("POST", "/input", r#"{"actions":[{"act":"draft","kit":1}]}"#);
    let s = p.json("POST", "/step", "");
    assert_eq!(s["tick"], json!(1));
    assert_eq!(s["last_acts"][0], json!({"act": "draft", "a": 1, "code": 0, "result": "ok"}));
    assert_eq!(s["view"]["me"]["kit"], json!(1));

    let s = p.json("POST", "/step", r#"{"actions":[{"act":"buy","item":1}, {"act":"buy","item":999}]}"#);
    assert_eq!(s["last_acts"][0]["result"], json!("ok"), "{s}");
    assert_eq!(s["last_acts"][1]["result"], json!("bad argument"), "the engine's answer reaches the page");
    let mut s = s;
    while s["view"]["match"]["phase"] != json!(PHASE_ACTION) {
        s = p.json("POST", "/step", "");
    }
    let x0 = s["view"]["me"]["x"].as_i64().unwrap();
    let (mx, my) = (s["view"]["me"]["x"].as_i64().unwrap(), s["view"]["me"]["y"].as_i64().unwrap());
    let aim = format!(r#"{{"move":[1,0],"aim":[{},{}],"fire":true}}"#, mx + 5000, my);
    for _ in 0..8 {
        s = p.json("POST", "/step", &aim);
    }
    assert_ne!(s["view"]["me"]["x"].as_i64().unwrap(), x0, "the unit moved: {}", s["view"]["me"]);
    let units = s["view"]["frame"]["units"].as_array().unwrap();
    assert!(units.iter().filter(|u| u["team"] == json!(0)).all(|u| u["credits"] == json!(0)), "enemy credits hidden");

    let out = p.quit();
    assert!(!out.replay.header.result.finished);
    assert_eq!(out.replay.header.bots[1].name, "human");
    assert_eq!(out.replay.header.bots[1].sha256, "", "no module ran for the player's team");
    let kinds = recorded(&out, me);
    for k in [A_DRAFT, A_BUY, A_MOVE, A_FACE, A_FIRE] {
        assert!(kinds.contains(&k), "act {k} recorded: {kinds:?}");
    }
    ibc_replay::verify(&out.replay).unwrap();
    let again = ibc_replay::Replay::decode(&out.replay.encode()).unwrap();
    ibc_replay::verify(&again).unwrap();
}

#[test]
fn realtime_runs_on_its_clock_once_started() {
    let p = start("realtime", Mode::Realtime { tps: 60 }, 0, true);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(p.json("GET", "/state", "")["tick"], json!(0), "starts paused");
    assert_eq!(p.json("POST", "/run", r#"{"running":true}"#)["running"], json!(true));
    p.json("POST", "/input", r#"{"move":[0,1],"walk":true}"#);
    let deadline = Instant::now() + Duration::from_secs(20);
    while p.session().state()["tick"].as_u64().unwrap() < 20 {
        assert!(Instant::now() < deadline, "the clock did not run");
        std::thread::sleep(Duration::from_millis(10));
    }
    p.json("POST", "/run", r#"{"running":false}"#);
    let out = p.quit();
    assert_eq!(out.replay.header.bots[0].name, "human+mates");
    assert_ne!(out.replay.header.bots[0].sha256, "", "the allies' module ran");
    ibc_replay::verify(&out.replay).unwrap();
}

#[test]
fn serves_the_page_and_rejects_what_it_must() {
    let p = start("http", Mode::Turn, 0, false);
    let page = p.call("GET", "/", "");
    assert_eq!((page.status, page.body.as_slice()), ("200 OK", &b"<!doctype html>play"[..]));
    for path in ["/../secret.txt", "/%2e%2e/secret.txt", "/missing.js"] {
        assert_ne!(p.call("GET", path, "").status, "200 OK", "{path}");
    }
    assert_eq!(p.call("POST", "/input", "{not json").status, "400 Bad Request");
    assert_eq!(p.call("POST", "/input", r#"{"teleport":[1,1]}"#).status, "400 Bad Request");
    assert_eq!(p.call("POST", "/input", "").status, "400 Bad Request");
    assert_eq!(p.call("POST", "/run", "{}").status, "400 Bad Request");
    assert_eq!(p.call("POST", "/nope", "{}").status, "404 Not Found");
    assert_eq!(p.call("PUT", "/state", "").status, "405 Method Not Allowed");
    let unmarked =
        Request { method: "POST".into(), path: "/step".into(), host: Some("localhost".into()), ..Request::default() };
    assert_eq!(respond(&p.site, &unmarked).status, "403 Forbidden", "POST without X-IBC-Play");
    let foreign = Request {
        method: "GET".into(),
        path: "/state".into(),
        host: Some("evil.example".into()),
        ..Request::default()
    };
    assert_eq!(respond(&p.site, &foreign).status, "403 Forbidden");
    assert_eq!(p.json("GET", "/state", "")["tick"], json!(0), "nothing above advanced the match");
    ibc_replay::verify(&p.quit().replay).unwrap();
}

#[test]
fn a_seat_outside_the_team_is_refused() {
    let err = prepare(Setup {
        map_text: MAP.into(),
        seed: 1,
        mode: Mode::Turn,
        team: 0,
        slot: 99,
        opponent: idle("o"),
        allies: None,
    })
    .err()
    .unwrap();
    assert!(err.to_string().contains("--unit 99"), "{err}");
}
