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
    BotSpec {
        name: name.into(),
        wasm: wat::parse_str(IDLE).unwrap(),
        fuel_multiplier: 1.0,
    }
}

/// Drafts kit 1 and walks right, every turn.
fn walker(name: &str) -> BotSpec {
    let wat = format!(
        r#"(module
  (import "ibc" "end_turn" (func $end_turn))
  (import "ibc" "act" (func $act (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "_start")
    (loop $l
      (drop (call $act (i32.const {A_DRAFT}) (i32.const 1) (i32.const 0) (i32.const 0)))
      (drop (call $act (i32.const {A_MOVE}) (i32.const 0) (i32.const 100) (i32.const 0)))
      (call $end_turn) (br $l))))"#
    );
    BotSpec {
        name: name.into(),
        wasm: wat::parse_str(wat).unwrap(),
        fuel_multiplier: 1.0,
    }
}

struct Running {
    site: Site,
    base: PathBuf,
    game: JoinHandle<MatchOutput>,
}

fn start(name: &str, mode: Mode, team: i32, slot: Option<u32>) -> Running {
    start_with_kit(name, mode, team, slot, None)
}

fn start_with_kit(
    name: &str,
    mode: Mode,
    team: i32,
    slot: Option<u32>,
    kit: Option<&str>,
) -> Running {
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
        slot,
        kit: kit.map(String::from),
        own: walker("mates"),
        opponent: idle("opponent"),
    })
    .unwrap();
    let s = Arc::clone(&session);
    let game = std::thread::spawn(move || ibc_runner::run_driven(cfg, &mut s.driver()).unwrap());
    let site = Site {
        root: base.join("dist").canonicalize().unwrap(),
        session,
    };
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
        assert_eq!(
            r.status,
            "200 OK",
            "{method} {path}: {}",
            String::from_utf8_lossy(&r.body)
        );
        serde_json::from_slice(&r.body).unwrap()
    }

    fn session(&self) -> &Session {
        &self.site.session
    }

    fn tick(&self) -> u64 {
        self.session().state()["tick"].as_u64().unwrap()
    }

    fn phase(&self) -> Value {
        self.session().state()["view"]["match"]["phase"].clone()
    }

    /// Wait (at most 20 s) until `done` holds.
    fn until(&self, what: &str, done: impl Fn(&Running) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The clock stays where it is for a while.
    fn assert_still(&self, why: &str) {
        let t = self.tick();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(self.tick(), t, "{why}");
    }

    fn quit(self) -> MatchOutput {
        self.json("POST", "/quit", "");
        let out = self.game.join().unwrap();
        std::fs::remove_dir_all(&self.base).unwrap();
        out
    }
}

/// The acts recorded in the replay for `unit` in the ticks `ticks`, by kind.
fn recorded(out: &MatchOutput, unit: u32, ticks: std::ops::Range<u64>) -> Vec<u32> {
    out.replay.ticks[ticks.start as usize..ticks.end as usize]
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
    let p = start("turn", Mode::Turn, 1, Some(1));
    let setup = p.json("GET", "/setup", "");
    let team_size = setup["team_size"].as_u64().unwrap() as u32;
    let me = team_size + 1;
    assert_eq!(
        (setup["team"].clone(), setup["mode"].clone()),
        (json!(1), json!("turn"))
    );
    assert_eq!(
        setup["first_unit"],
        json!(team_size),
        "team B starts after team A"
    );
    assert!(setup["kits"].as_array().unwrap().len() > 1 && setup["map_text"].is_string());

    let before = p.json("GET", "/state", "");
    assert_eq!(
        (
            before["tick"].clone(),
            before["view"]["match"]["phase"].clone()
        ),
        (json!(0), json!(PHASE_DRAFT))
    );
    assert_eq!(
        (
            before["possessed"].clone(),
            before["view"]["me"]["id"].clone()
        ),
        (json!(1), json!(me))
    );
    // The draft is the bot's, even for the possessed unit.
    let draft = p.call("POST", "/input", r#"{"actions":[{"act":"draft","kit":2}]}"#);
    assert_eq!(draft.status, "400 Bad Request", "the player does not draft");
    let mut s = p.json("POST", "/step", "");
    while s["view"]["match"]["phase"] == json!(PHASE_DRAFT) {
        s = p.json("POST", "/step", "");
    }
    let draft_ticks = s["tick"].as_u64().unwrap();

    let s = p.json(
        "POST",
        "/step",
        r#"{"actions":[{"act":"buy","item":1}, {"act":"buy","item":999}]}"#,
    );
    assert_eq!(s["last_acts"][0]["result"], json!("ok"), "{s}");
    assert_eq!(
        s["last_acts"][1]["result"],
        json!("bad argument"),
        "the engine's answer reaches the page"
    );
    let mut s = s;
    while s["view"]["match"]["phase"] != json!(PHASE_ACTION) {
        s = p.json("POST", "/step", "");
    }
    let x0 = s["view"]["me"]["x"].as_i64().unwrap();
    let (mx, my) = (x0, s["view"]["me"]["y"].as_i64().unwrap());
    let aim = format!(
        r#"{{"move":[1,0],"aim":[{},{}],"fire":true}}"#,
        mx + 5000,
        my
    );
    for _ in 0..8 {
        s = p.json("POST", "/step", &aim);
    }
    assert_ne!(
        s["view"]["me"]["x"].as_i64().unwrap(),
        x0,
        "the unit moved: {}",
        s["view"]["me"]
    );
    let units = s["view"]["frame"]["units"].as_array().unwrap();
    assert!(
        units
            .iter()
            .filter(|u| u["team"] == json!(0))
            .all(|u| u["credits"] == json!(0)),
        "enemy credits hidden"
    );

    // Possessed and idle, then given back: the bot walks again only once it has the unit back.
    let t0 = s["tick"].as_u64().unwrap();
    p.json(
        "POST",
        "/input",
        r#"{"move":[0,0],"fire":false,"aim":null}"#,
    );
    for _ in 0..4 {
        p.json("POST", "/step", "");
    }
    let t1 = p.tick();
    let watching = p.json("POST", "/possess", r#"{"slot":null}"#);
    assert_eq!(
        (
            watching["possessed"].clone(),
            watching["view"]["me"].clone()
        ),
        (Value::Null, Value::Null)
    );
    for _ in 0..4 {
        p.json("POST", "/step", "");
    }
    let t2 = p.tick();
    assert_eq!(
        p.call("POST", "/possess", &format!(r#"{{"slot":{team_size}}}"#))
            .status,
        "400 Bad Request"
    );

    let out = p.quit();
    assert!(!out.replay.header.result.finished);
    assert_eq!(out.replay.header.bots[1].name, "human+mates");
    assert_ne!(
        out.replay.header.bots[1].sha256, "",
        "the team's module ran"
    );
    let drafts: Vec<i32> = out.replay.ticks[..draft_ticks as usize]
        .iter()
        .flat_map(|t| &t.events)
        .filter_map(|e| match e {
            Event::Input(ibc_engine::Input::Act {
                unit,
                kind: A_DRAFT,
                a,
                ..
            }) if *unit == me => Some(*a),
            _ => None,
        })
        .collect();
    assert!(
        !drafts.is_empty() && drafts.iter().all(|&k| k == 1),
        "the team's bot drafted: {drafts:?}"
    );
    let kinds = recorded(&out, me, draft_ticks..t0);
    for k in [A_BUY, A_MOVE, A_FACE, A_FIRE] {
        assert!(kinds.contains(&k), "act {k} recorded: {kinds:?}");
    }
    assert!(
        recorded(&out, me, t0..t1).is_empty(),
        "no bot acts while possessed"
    );
    assert!(
        recorded(&out, team_size, t0..t1).contains(&A_MOVE),
        "the teammate's bot kept playing"
    );
    assert!(
        recorded(&out, me, t1..t2).contains(&A_MOVE),
        "the bot has the unit back"
    );
    ibc_replay::verify(&out.replay).unwrap();
    let again = ibc_replay::Replay::decode(&out.replay.encode()).unwrap();
    ibc_replay::verify(&again).unwrap();
}

#[test]
fn active_mode_passes_time_only_while_the_player_acts() {
    let p = start("active", Mode::Active { tps: 60 }, 0, Some(0));
    // The bots' draft runs on its own, and time stops in the buy phase.
    p.until("the buy phase", |p| p.phase() == json!(PHASE_BUY));
    p.assert_still("nothing held in the buy phase: no ticks");
    p.json(
        "POST",
        "/input",
        r#"{"move":[1,0],"actions":[{"act":"buy","item":1}]}"#,
    );
    p.assert_still("moving and buying take no ticks in the buy phase");
    let t = p.tick();
    let s = p.json("POST", "/step", "");
    assert_eq!(s["tick"].as_u64().unwrap(), t + 1, "Space passes one tick");
    assert_eq!(
        s["last_acts"][0]["act"],
        json!("buy"),
        "the queued buy went out with it"
    );
    p.assert_still("and only one");

    p.json("POST", "/input", r#"{"wait":true}"#);
    p.until("the action phase", |p| p.phase() == json!(PHASE_ACTION));
    p.json("POST", "/input", r#"{"wait":false,"move":[0,0]}"#);
    p.assert_still("idle in the action phase");
    let t = p.tick();
    p.json("POST", "/input", r#"{"move":[0,1]}"#);
    p.until("ticks while moving", |p| p.tick() >= t + 5);
    p.json("POST", "/input", r#"{"move":[0,0]}"#);
    p.assert_still("stopped moving");
    let t = p.tick();
    let s = p.json("GET", &format!("/state?after={t}"), "");
    assert_eq!(
        s["tick"].as_u64().unwrap(),
        t,
        "a long poll with nothing happening answers after its wait"
    );
    p.json("POST", "/run", r#"{"running":true}"#);
    p.until("free-running time", |p| p.tick() >= t + 5);
    p.json("POST", "/run", r#"{"running":false}"#);
    ibc_replay::verify(&p.quit().replay).unwrap();
}

#[test]
fn realtime_runs_on_its_clock_once_started() {
    let p = start("realtime", Mode::Realtime { tps: 60 }, 0, None);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        p.json("GET", "/state", "")["tick"],
        json!(0),
        "starts paused"
    );
    assert_eq!(
        p.json("GET", "/state", "")["view"]["me"],
        Value::Null,
        "watching until a unit is chosen"
    );
    assert_eq!(
        p.json("POST", "/run", r#"{"running":true}"#)["running"],
        json!(true)
    );
    p.json("POST", "/possess", r#"{"slot":2}"#);
    p.json("POST", "/input", r#"{"move":[0,1],"walk":true}"#);
    p.until("the clock", |p| p.tick() >= 20);
    let s = p.json("GET", "/state?after=25", "");
    assert!(
        s["tick"].as_u64().unwrap() > 25,
        "the long poll answered with a newer tick: {}",
        s["tick"]
    );
    p.json("POST", "/run", r#"{"running":false}"#);
    let out = p.quit();
    assert_eq!(out.replay.header.bots[0].name, "human+mates");
    ibc_replay::verify(&out.replay).unwrap();
}

#[test]
fn serves_the_page_and_rejects_what_it_must() {
    let p = start("http", Mode::Turn, 0, None);
    let page = p.call("GET", "/", "");
    assert_eq!(
        (page.status, page.body.as_slice()),
        ("200 OK", &b"<!doctype html>play"[..])
    );
    for path in ["/../secret.txt", "/%2e%2e/secret.txt", "/missing.js"] {
        assert_ne!(p.call("GET", path, "").status, "200 OK", "{path}");
    }
    assert_eq!(
        p.call("POST", "/input", "{not json").status,
        "400 Bad Request"
    );
    assert_eq!(
        p.call("POST", "/input", r#"{"teleport":[1,1]}"#).status,
        "400 Bad Request"
    );
    assert_eq!(p.call("POST", "/input", "").status, "400 Bad Request");
    assert_eq!(p.call("POST", "/run", "{}").status, "400 Bad Request");
    assert_eq!(p.call("POST", "/possess", "{}").status, "400 Bad Request");
    assert_eq!(
        p.call("POST", "/possess", r#"{"slot":-1}"#).status,
        "400 Bad Request"
    );
    assert_eq!(
        p.call("GET", "/state?after=x", "").status,
        "400 Bad Request"
    );
    assert_eq!(p.call("POST", "/nope", "{}").status, "404 Not Found");
    assert_eq!(p.call("PUT", "/state", "").status, "405 Method Not Allowed");
    let unmarked = Request {
        method: "POST".into(),
        path: "/step".into(),
        host: Some("localhost".into()),
        ..Request::default()
    };
    assert_eq!(
        respond(&p.site, &unmarked).status,
        "403 Forbidden",
        "POST without X-IBC-Play"
    );
    let foreign = Request {
        method: "GET".into(),
        path: "/state".into(),
        host: Some("evil.example".into()),
        ..Request::default()
    };
    assert_eq!(respond(&p.site, &foreign).status, "403 Forbidden");
    assert_eq!(
        p.json("GET", "/state", "")["tick"],
        json!(0),
        "nothing above advanced the match"
    );
    ibc_replay::verify(&p.quit().replay).unwrap();
}

#[test]
fn a_seat_outside_the_team_is_refused() {
    let err = prepare(Setup {
        map_text: MAP.into(),
        seed: 1,
        mode: Mode::Turn,
        team: 0,
        slot: Some(99),
        kit: None,
        own: idle("m"),
        opponent: idle("o"),
    })
    .err()
    .unwrap();
    assert!(err.to_string().contains("--unit 99"), "{err}");
}

#[test]
fn a_chosen_kit_replaces_the_bots_draft() {
    let kit = ibc_engine::Ruleset::builtin().kit_names[2].clone();
    let p = start_with_kit("kit", Mode::Turn, 0, Some(0), Some(&kit));
    let mut s = p.json("POST", "/step", "");
    while s["view"]["match"]["phase"] == json!(PHASE_DRAFT) {
        s = p.json("POST", "/step", "");
    }
    assert_eq!(
        s["view"]["me"]["kit"],
        json!(2),
        "the walker drafts kit 1, the chosen kit wins: {}",
        s["view"]["me"]
    );
    ibc_replay::verify(&p.quit().replay).unwrap();

    let err = prepare(Setup {
        map_text: MAP.into(),
        seed: 1,
        mode: Mode::Turn,
        team: 0,
        slot: Some(0),
        kit: Some("no-such-kit".into()),
        own: idle("m"),
        opponent: idle("o"),
    })
    .err()
    .unwrap();
    assert!(
        err.to_string().contains(&kit),
        "the error lists the kits: {err}"
    );
}
