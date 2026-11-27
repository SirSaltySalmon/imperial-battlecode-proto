//! Fuel, fault and determinism semantics of the runner, checked with tiny hand-written WAT bots.

use ibc_engine::Ruleset;
use ibc_runner::{run_match, scaled_budget, BotSpec, MatchConfig, MatchOutput};

const MAP: &str = include_str!("../../../maps/crossfire.ibcmap");
const TICKS: u32 = 40;
/// Longest check-free path of the test bots, in fuel: how far a unit may run past its budget.
const SLACK: u64 = 64;

/// Ends its turn immediately, forever.
const IDLE: &str = r#"(module
  (import "ibc" "end_turn" (func $end_turn))
  (memory (export "memory") 1)
  (func (export "_start") (loop $l (call $end_turn) (br $l))))"#;

/// Never calls end_turn. Counts in a local, and every 4096 iterations writes the counter (4 raw bytes) to
/// stdout, so the log shows whether the computation was resumed or restarted.
const SPINNER: &str = r#"(module
  (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 16) "\00\00\00\00\04\00\00\00")  ;; iovec { ptr = 0, len = 4 }
  (func (export "_start") (local $i i32)
    (loop $l
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (if (i32.eqz (i32.and (local.get $i) (i32.const 4095)))
        (then
          (i32.store (i32.const 0) (local.get $i))
          (drop (call $fd_write (i32.const 1) (i32.const 16) (i32.const 1) (i32.const 32)))))
      (br $l))))"#;

/// Calls `ibc.get(T_SELF)` in a tight loop and logs one byte after every call. Never calls end_turn.
const GETTER: &str = r#"(module
  (import "ibc" "get" (func $get (param i32 i32 i32 i32) (result i32)))
  (import "ibc" "log" (func $log (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 8) "\n")
  (func (export "_start")
    (loop $l
      (drop (call $get (i32.const 1) (i32.const 0) (i32.const 1024) (i32.const 512)))
      (drop (call $log (i32.const 8) (i32.const 1)))
      (br $l))))"#;

/// Plays two turns, then hits `unreachable`.
const CRASHER: &str = r#"(module
  (import "ibc" "end_turn" (func $end_turn))
  (memory (export "memory") 1)
  (func (export "_start")
    (call $end_turn)
    (call $end_turn)
    (unreachable)))"#;

/// Moves every tick (so the replay has inputs), checks `fuel_remaining` and uses the WASI shim.
const MOVER: &str = r#"(module
  (import "ibc" "end_turn" (func $end_turn))
  (import "ibc" "act" (func $act (param i32 i32 i32 i32) (result i32)))
  (import "ibc" "fuel_remaining" (func $fuel (result i64)))
  (import "ibc" "mem_write" (func $mem_write (param i32 i32 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "random_get" (func $random (param i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "clock_time_get" (func $clock (param i32 i64 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "_start") (local $before i64)
    (loop $l
      (drop (call $random (i32.const 64) (i32.const 4)))
      (drop (call $clock (i32.const 0) (i64.const 0) (i32.const 72)))
      (local.set $before (call $fuel))
      ;; A_MOVE with a random angle, speed 300
      (drop (call $act (i32.const 1) (i32.and (i32.load (i32.const 64)) (i32.const 65535)) (i32.const 300) (i32.const 0)))
      ;; the act must have cost at least FUEL_ACT (20) of this turn's budget
      (if (i64.gt_s (call $fuel) (i64.sub (local.get $before) (i64.const 20))) (then (unreachable)))
      ;; share the clock with the team
      (drop (call $mem_write (i32.const 0) (i32.const 0) (i32.const 72) (i32.const 8)))
      (call $end_turn)
      (br $l))))"#;

fn bot(name: &str, wat: &str) -> BotSpec {
    BotSpec { name: name.into(), wasm: wat::parse_str(wat).unwrap(), fuel_multiplier: 1.0 }
}

fn run(a: BotSpec, b: BotSpec) -> MatchOutput {
    run_match(MatchConfig {
        rules: Ruleset::builtin(),
        map_text: MAP.into(),
        seed: 42,
        bots: [a, b],
        log_limit: 8 << 20,
        max_ticks: Some(TICKS),
        frames: false,
    })
    .unwrap()
}

/// Sum of the engine's budgets over the ticks of the run, for a unit that is scheduled every tick.
fn granted_per_unit(out: &MatchOutput, unit: usize) -> u64 {
    assert_eq!(out.units[unit].turns, TICKS as u64, "unit {unit} should be scheduled every tick");
    out.units[unit].fuel_granted
}

#[test]
fn spinner_uses_exactly_its_budget_and_resumes() {
    let out = run(bot("spinner", SPINNER), bot("idle", IDLE));
    assert_eq!(out.summary.ticks, TICKS, "match advances although team A never ends a turn");
    let per_tick = scaled_budget(100_000, 1.0).unwrap();
    for (id, u) in out.units.iter().enumerate() {
        if u.team != 0 {
            assert_eq!(u.out_of_fuel_turns, 0);
            assert!(u.fuel_used < u.turns * 100, "idle bot burns almost nothing: {}", u.fuel_used);
            continue;
        }
        let granted = granted_per_unit(&out, id);
        assert!(granted >= TICKS as u64 * per_tick);
        // Exactly the budget: never less, and at most one loop iteration more over the whole match.
        assert!(u.fuel_used >= granted && u.fuel_used <= granted + SLACK, "used {} granted {granted}", u.fuel_used);
        assert_eq!(u.out_of_fuel_turns, TICKS as u64);
        assert!(!u.faulted);

        // The counter values written to stdout: strictly increasing by 4096, never restarting.
        let log = &out.logs[id];
        assert_eq!(log.len() % 4, 0);
        let values: Vec<u32> = log.chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
        assert!(values.len() > 2 * TICKS as usize, "only {} checkpoints", values.len());
        for (i, v) in values.iter().enumerate() {
            assert_eq!(*v, (i as u32 + 1) * 4096);
        }
        // More iterations than one tick's budget could pay for: the loop spans ticks.
        assert!(*values.last().unwrap() as u64 * 4 > 2 * per_tick);
    }
    let fuel = &out.summary.teams[0].fuel;
    assert_eq!(fuel.out_of_fuel_turns, fuel.turns);
    assert!(fuel.total >= fuel.granted && fuel.total <= fuel.granted + 5 * SLACK);
    // Every spinner turn uses its whole budget, and the smallest budget is an action tick's.
    assert!(fuel.turn_p50 >= per_tick && fuel.turn_p50 <= fuel.turn_p90 && fuel.turn_p90 <= fuel.turn_p99);
    assert!(fuel.turn_p99 <= fuel.max_unit_turn, "{fuel:?}");
    let idle = &out.summary.teams[1].fuel;
    assert!(idle.turn_p99 < 100 && idle.turn_p99 <= idle.max_unit_turn, "{idle:?}");
}

#[test]
fn every_turn_is_capped_at_its_budget() {
    // Budgets differ per phase (first tick x10, buy x4); a spinner must never carry a big slice over.
    let out = run(bot("spinner", SPINNER), bot("idle", IDLE));
    for u in out.units.iter().filter(|u| u.team == 0) {
        // the largest turn is the draft tick (10x); nothing may exceed it
        assert!(u.max_turn <= 1_000_000 + SLACK, "max turn {}", u.max_turn);
    }
}

#[test]
fn host_calls_are_paid_from_the_same_budget() {
    let out = run(bot("getter", GETTER), bot("idle", IDLE));
    assert_eq!(out.summary.ticks, TICKS);
    for (id, u) in out.units.iter().enumerate().filter(|(_, u)| u.team == 0) {
        let granted = granted_per_unit(&out, id);
        let calls = out.logs[id].len() as u64;
        assert!(out.logs[id].iter().all(|&b| b == b'\n'));
        assert!(calls > 1000, "the bot did get to call the host ({calls} calls)");
        // wasm + host charges never exceed the budget in total, nor per turn (via max_turn) by more than the
        // wasm between a charge and the next fuel check, which the next turn pays back
        assert!(u.fuel_used <= granted, "used {} granted {granted}", u.fuel_used);
        assert!(u.max_turn <= 1_000_000 + SLACK, "max turn {}", u.max_turn);
        // every call was charged in full
        assert!(calls * ibc_abi::FUEL_GET <= u.fuel_used, "{calls} calls for {} fuel", u.fuel_used);
        // and nearly all of the budget was usable (at most one unaffordable call's worth lost per turn)
        assert!(u.fuel_used + TICKS as u64 * 64 >= granted, "used {} granted {granted}", u.fuel_used);
        assert_eq!(u.out_of_fuel_turns, TICKS as u64);
    }
}

#[test]
fn fuel_multiplier_scales_the_budget() {
    let mut a = bot("spinner", SPINNER);
    a.fuel_multiplier = 2.5;
    let fast = run(a, bot("idle", IDLE));
    let base = run(bot("spinner", SPINNER), bot("idle", IDLE));
    let (f, b) = (&fast.summary.teams[0].fuel, &base.summary.teams[0].fuel);
    assert_eq!(f.granted * 2, b.granted * 5);
    assert!(f.total >= f.granted && f.total <= f.granted + 5 * SLACK);
    assert_eq!(fast.replay.header.bots[0].fuel_multiplier, 2.5);
}

#[test]
fn a_trap_faults_only_its_unit() {
    let out = run(bot("crasher", CRASHER), bot("mover", MOVER));
    assert_eq!(out.summary.ticks, TICKS, "the match continues");
    let team_a: Vec<usize> = (0..out.units.len()).filter(|&i| out.units[i].team == 0).collect();
    for (id, u) in out.units.iter().enumerate() {
        assert_eq!(u.faulted, u.team == 0, "unit {id}");
        if u.team == 0 {
            assert_eq!(u.turns, 3, "two turns, then the trap");
            let log = String::from_utf8_lossy(&out.logs[id]);
            assert!(log.contains("faulted at tick 2") && log.contains("unreachable"), "{log}");
        } else {
            assert_eq!(u.turns, TICKS as u64);
            assert_eq!(u.out_of_fuel_turns, 0);
        }
    }
    assert_eq!(out.summary.faults.len(), team_a.len());
    assert_eq!(out.summary.teams[0].faults as usize, team_a.len());
    assert_eq!(out.summary.teams[1].faults, 0);
    assert!(out.summary.faults.iter().all(|f| f.team == 0 && f.tick == 2 && f.reason.contains("unreachable")));
    // faults are part of the replay, so it still verifies
    ibc_replay::verify(&out.replay).unwrap();
}

#[test]
fn same_match_twice_is_byte_identical() {
    let a = run(bot("mover", MOVER), bot("spinner", SPINNER));
    let b = run(bot("mover", MOVER), bot("spinner", SPINNER));
    assert!(a.replay.ticks.iter().any(|t| !t.events.is_empty()), "the mover produced inputs");
    assert!(a.summary.faults.is_empty(), "{:?}", a.summary.faults);
    assert_eq!(a.replay.encode(), b.replay.encode());
    assert_eq!(a.summary, b.summary);
    assert_eq!(a.summary.to_json(), b.summary.to_json());
    assert_eq!(a.logs, b.logs);
    assert_eq!(a.units, b.units);
    assert_eq!(a.summary.replay_sha256, ibc_replay::sha256_hex(&a.replay.encode()));
    // The bytes the runner hands out for `replay.ibcr` are that encoding.
    assert_eq!(a.replay_bytes, a.replay.encode());
}

#[test]
fn replay_verifies_and_roundtrips() {
    let out = run(bot("mover", MOVER), bot("getter", GETTER));
    ibc_replay::verify(&out.replay).unwrap();
    let decoded = ibc_replay::Replay::decode(&out.replay.encode()).unwrap();
    assert_eq!(decoded, out.replay);
    ibc_replay::verify(&decoded).unwrap();
    assert_eq!(decoded.header.wasmtime_version, ibc_runner::WASMTIME_VERSION);
    assert_eq!(decoded.header.result.ticks, TICKS);
}

#[test]
fn frames_are_collected_on_request() {
    let out = run_match(MatchConfig {
        rules: Ruleset::builtin(),
        map_text: MAP.into(),
        seed: 1,
        bots: [bot("idle", IDLE), bot("idle", IDLE)],
        log_limit: 1024,
        max_ticks: Some(5),
        frames: true,
    })
    .unwrap();
    let frames = out.frames.unwrap();
    assert_eq!(frames.len(), 6, "initial state + one per tick");
    // the same frames come out of the replay
    let mut again = Vec::new();
    out.replay.simulate(|m| again.push(m.frame_json())).unwrap();
    assert_eq!(frames, again);
}

#[test]
fn unsupported_imports_fail_the_load_with_a_list() {
    let bad = r#"(module
      (import "wasi_snapshot_preview1" "sock_accept" (func (param i32 i32 i32) (result i32)))
      (import "env" "whatever" (func))
      (import "ibc" "end_turn" (func))
      (memory (export "memory") 1)
      (func (export "_start")))"#;
    let err = ibc_runner::check_wasm(&wat::parse_str(bad).unwrap()).unwrap_err().to_string();
    assert!(err.contains("wasi_snapshot_preview1.sock_accept") && err.contains("env.whatever"), "{err}");
    assert!(!err.contains("end_turn"), "{err}");
    ibc_runner::check_wasm(&wat::parse_str(MOVER).unwrap()).unwrap();
}

#[test]
fn log_is_capped_but_writes_succeed() {
    let out = run_match(MatchConfig {
        rules: Ruleset::builtin(),
        map_text: MAP.into(),
        seed: 1,
        bots: [bot("getter", GETTER), bot("idle", IDLE)],
        log_limit: 100,
        max_ticks: Some(5),
        frames: false,
    })
    .unwrap();
    for (id, u) in out.units.iter().enumerate().filter(|(_, u)| u.team == 0) {
        assert!(!u.faulted);
        let log = String::from_utf8_lossy(&out.logs[id]);
        assert!(log.starts_with(&"\n".repeat(100)) && log.contains("log truncated at 100 bytes"), "{log:?}");
    }
}
