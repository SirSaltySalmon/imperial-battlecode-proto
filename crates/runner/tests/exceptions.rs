//! Wasm exception handling, in the standard (exnref) encoding C++ bots built with exceptions use: a bot
//! that throws and catches keeps playing, fuel stays exact across throws and catches (also when the budget
//! runs out inside a catch handler), an uncaught exception faults only its unit, and the legacy encoding
//! is refused. The fuel model itself is covered by tests/fuel.rs.

use ibc_engine::Ruleset;
use ibc_replay::Record;
use ibc_runner::{run_match, BotSpec, MatchConfig, MatchOutput};

const MAP: &str = include_str!("../../../maps/crossfire.ibcmap");
const TICKS: u32 = 40;
/// Longest check-free path of the test bots, in fuel: how far a unit may run past its budget.
const SLACK: u64 = 64;

/// Throws and catches across calls every turn (see the file). The coordinator's upload check accepts the
/// same module (crates/coordinator/src/wasmcheck.rs).
const THROWER: &str = include_str!("../../../fixtures/exceptions/thrower.wat");
/// The legacy `try`/`catch` encoding, refused here and by the coordinator.
const LEGACY: &str = include_str!("../../../fixtures/exceptions/legacy.wat");

const IDLE: &str = r#"(module
  (import "ibc" "end_turn" (func $end_turn))
  (memory (export "memory") 1)
  (func (export "_start") (loop $l (call $end_turn) (br $l))))"#;

/// Never ends its turn. Every iteration throws its counter plus one from `$throw` and catches it in the
/// loop; every 4096 iterations it writes the counter (4 raw bytes) to stdout. The counter only survives
/// through the exception payload, so the log shows whether each throw arrived and whether the loop was
/// resumed or restarted.
const THROW_SPINNER: &str = r#"(module
  (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 16) "\00\00\00\00\04\00\00\00")
  (tag $up (param i32))
  (func $throw (param $v i32) (throw $up (local.get $v)))
  (func (export "_start") (local $i i32)
    (loop $l
      (local.set $i
        (block $caught (result i32)
          (try_table (catch $up $caught) (call $throw (i32.add (local.get $i) (i32.const 1))))
          (unreachable)))
      (if (i32.eqz (i32.and (local.get $i) (i32.const 4095)))
        (then
          (i32.store (i32.const 0) (local.get $i))
          (drop (call $fd_write (i32.const 1) (i32.const 16) (i32.const 1) (i32.const 32)))))
      (br $l))))"#;

/// Iterations of `HANDLER_WORKER`'s catch handler, 8 fuel each: about 250k fuel, more than a turn's budget
/// in a round (100k) and less than in the buy phase (4 x) or on the draft tick (10 x).
const HANDLER_LOOPS: u32 = 31_250;

/// Each turn throws `HANDLER_LOOPS`, and its catch handler counts to the thrown number before it writes the
/// count to stdout and ends its turn.
const HANDLER_WORKER: &str = r#"(module
  (import "ibc" "end_turn" (func $end_turn))
  (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 16) "\00\00\00\00\04\00\00\00")
  (tag $work (param i32))
  (func $throw (throw $work (i32.const HANDLER_LOOPS)))
  (func (export "_start") (local $n i32) (local $k i32)
    (loop $turn
      (local.set $n
        (block $caught (result i32)
          (try_table (catch $work $caught) (call $throw))
          (unreachable)))
      ;; the catch handler
      (local.set $k (i32.const 0))
      (loop $count
        (local.set $k (i32.add (local.get $k) (i32.const 1)))
        (br_if $count (i32.lt_u (local.get $k) (local.get $n))))
      (i32.store (i32.const 0) (local.get $k))
      (drop (call $fd_write (i32.const 1) (i32.const 16) (i32.const 1) (i32.const 32)))
      (call $end_turn)
      (br $turn))))"#;

/// Writes, every turn, the fuel one throw and catch took (8 bytes): `fuel_remaining` before and after.
const THROW_COST: &str = r#"(module
  (import "ibc" "end_turn" (func $end_turn))
  (import "ibc" "fuel_remaining" (func $fuel (result i64)))
  (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 16) "\00\00\00\00\08\00\00\00")
  (tag $e (param i32))
  (func $throw (throw $e (i32.const 1)))
  (func (export "_start") (local $before i64)
    (loop $turn
      (local.set $before (call $fuel))
      (drop
        (block $caught (result i32)
          (try_table (catch $e $caught) (call $throw))
          (unreachable)))
      (i64.store (i32.const 0) (i64.sub (local.get $before) (call $fuel)))
      (drop (call $fd_write (i32.const 1) (i32.const 16) (i32.const 1) (i32.const 32)))
      (call $end_turn)
      (br $turn))))"#;

/// Plays two turns, then throws an exception nothing catches.
const UNCAUGHT: &str = r#"(module
  (import "ibc" "end_turn" (func $end_turn))
  (memory (export "memory") 1)
  (tag $e (param i32))
  (func (export "_start")
    (call $end_turn)
    (call $end_turn)
    (throw $e (i32.const 7))))"#;

fn bot(name: &str, wat: &str) -> BotSpec {
    BotSpec {
        name: name.into(),
        wasm: wat::parse_str(wat).unwrap(),
        fuel_multiplier: 1.0,
    }
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

fn team_a(out: &MatchOutput) -> Vec<usize> {
    (0..out.units.len())
        .filter(|&i| out.units[i].team == 0)
        .collect()
}

/// A log of raw little-endian `u32`s.
fn u32s(log: &[u8]) -> Vec<u32> {
    assert_eq!(log.len() % 4, 0, "log is not a sequence of u32s");
    log.chunks(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

/// (fuel used, ran out of fuel) of every turn of `unit`, in order, from the replay.
fn turns(out: &MatchOutput, unit: usize) -> Vec<(u64, bool)> {
    let team = out.units[unit].team as usize;
    let mut turns = Vec::new();
    for tick in &out.replay.ticks {
        for record in &tick.private[team] {
            if let Record::Fuel {
                unit: u,
                used,
                out_of_fuel,
            } = record
            {
                if *u as usize == unit {
                    turns.push((*used, *out_of_fuel));
                }
            }
        }
    }
    turns
}

#[test]
fn a_bot_that_throws_and_catches_keeps_playing() {
    let out = run(bot("thrower", THROWER), bot("idle", IDLE));
    assert_eq!(out.summary.ticks, TICKS);
    assert!(out.summary.faults.is_empty(), "{:?}", out.summary.faults);
    for id in team_a(&out) {
        let u = &out.units[id];
        assert_eq!(
            (u.turns, u.out_of_fuel_turns),
            (TICKS as u64, 0),
            "unit {id}"
        );
        // one throw caught per turn, its payload (3) delivered every time
        let expected: Vec<u32> = (1..=TICKS).map(|t| 3 * t).collect();
        assert_eq!(u32s(&out.logs[id]), expected, "unit {id}");
        // every turn after the first does the same work, so it costs the same fuel
        let used: Vec<u64> = turns(&out, id).iter().map(|t| t.0).collect();
        assert!(
            used[1..].iter().all(|&f| f == used[1]),
            "unit {id}: {used:?}"
        );
    }
    assert!(
        out.replay.ticks.iter().any(|t| !t.events.is_empty()),
        "the thrower moved"
    );
    ibc_replay::verify(&out.replay).unwrap();
    let again = run(bot("thrower", THROWER), bot("idle", IDLE));
    assert_eq!(out.replay.encode(), again.replay.encode());
    assert_eq!(out.units, again.units);
}

#[test]
fn fuel_stays_exact_across_throws_and_catches() {
    let out = run(bot("throw-spinner", THROW_SPINNER), bot("idle", IDLE));
    assert_eq!(
        out.summary.ticks, TICKS,
        "match advances although team A never ends a turn"
    );
    for id in team_a(&out) {
        let u = &out.units[id];
        assert_eq!(
            (u.turns, u.out_of_fuel_turns),
            (TICKS as u64, TICKS as u64),
            "unit {id}"
        );
        assert!(!u.faulted);
        // exactly the budget: never less, and at most one loop iteration more over the whole match
        let granted = u.fuel_granted;
        assert!(
            u.fuel_used >= granted && u.fuel_used <= granted + SLACK,
            "used {} granted {granted}",
            u.fuel_used
        );
        // every throw arrived and the loop was resumed, never restarted
        let values = u32s(&out.logs[id]);
        assert!(
            values.len() > TICKS as usize,
            "only {} checkpoints",
            values.len()
        );
        for (i, v) in values.iter().enumerate() {
            assert_eq!(*v, (i as u32 + 1) * 4096, "unit {id}");
        }
    }
}

#[test]
fn running_out_of_fuel_inside_a_catch_handler_resumes_there() {
    let worker = HANDLER_WORKER.replace("HANDLER_LOOPS", &HANDLER_LOOPS.to_string());
    let out = run(bot("handler-worker", &worker), bot("idle", IDLE));
    assert!(out.summary.faults.is_empty(), "{:?}", out.summary.faults);
    for id in team_a(&out) {
        assert!(out.units[id].out_of_fuel_turns > 0, "unit {id}");
        // every handler finished its count: resumed where it ran out, never restarted
        let done = u32s(&out.logs[id]);
        assert!(
            done.iter().all(|&k| k == HANDLER_LOOPS),
            "unit {id}: {done:?}"
        );

        // Split the turns into iterations of the turn loop (each ends with the turn that called end_turn).
        // The first iteration also pays for entering `_start`; the last may be unfinished.
        let mut iterations: Vec<(u64, usize)> = Vec::new(); // (fuel, turns it took)
        let mut current = (0, 0);
        for (used, out_of_fuel) in turns(&out, id) {
            current = (current.0 + used, current.1 + 1);
            if !out_of_fuel {
                iterations.push(current);
                current = (0, 0);
            }
        }
        assert_eq!(iterations.len(), done.len());
        let whole = &iterations[1..];
        // A buy-phase turn pays for a whole iteration; in a round the handler runs out of fuel and the
        // iteration is spread over several turns, suspended inside the handler each time...
        assert!(
            whole.iter().any(|it| it.1 == 1),
            "unit {id}: {iterations:?}"
        );
        assert!(
            whole.iter().filter(|it| it.1 >= 3).count() >= 3,
            "unit {id}: {iterations:?}"
        );
        // ...and every iteration costs exactly the same fuel either way.
        assert!(
            whole.iter().all(|it| it.0 == whole[0].0),
            "unit {id}: {iterations:?}"
        );
    }
}

#[test]
fn a_throw_and_catch_costs_one_fuel_per_instruction() {
    let out = run(bot("throw-cost", THROW_COST), bot("idle", IDLE));
    for id in team_a(&out) {
        let log = &out.logs[id];
        let costs: Vec<u64> = log
            .chunks(8)
            .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(costs.len(), TICKS as usize);
        // local.set, try_table, call, entering `$throw`, i32.const, throw, then i32.const, local.get and the
        // call that reads the fuel again: 1 each. Unwinding, the catch and the exception object are free.
        assert!(costs.iter().all(|&c| c == 9), "unit {id}: {costs:?}");
    }
}

#[test]
fn an_uncaught_exception_faults_only_its_unit() {
    let out = run(bot("uncaught", UNCAUGHT), bot("thrower", THROWER));
    assert_eq!(out.summary.ticks, TICKS, "the match continues");
    let team = team_a(&out);
    assert_eq!(out.summary.faults.len(), team.len());
    assert!(
        out.summary
            .faults
            .iter()
            .all(|f| f.team == 0 && f.tick == 2 && f.reason == "uncaught wasm exception"),
        "{:?}",
        out.summary.faults
    );
    for id in team {
        let log = String::from_utf8_lossy(&out.logs[id]);
        assert!(
            log.contains("faulted at tick 2: uncaught wasm exception"),
            "{log}"
        );
    }
    for (id, u) in out.units.iter().enumerate().filter(|(_, u)| u.team == 1) {
        assert!(!u.faulted && u.turns == TICKS as u64, "unit {id}");
    }
    ibc_replay::verify(&out.replay).unwrap();
}

#[test]
fn only_the_standard_encoding_is_accepted() {
    ibc_runner::check_wasm(&wat::parse_str(THROWER).unwrap()).unwrap();
    let err = ibc_runner::check_wasm(&wat::parse_str(LEGACY).unwrap()).unwrap_err();
    let err = format!("{err:#}");
    assert!(
        err.contains("invalid wasm") && err.contains("legacy_exceptions feature required"),
        "{err}"
    );
}
