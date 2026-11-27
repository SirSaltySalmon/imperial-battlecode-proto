//! What bots print: stdout and stderr go through the WASI shim's `fd_write` into the unit log, next to
//! `ibc.log`, capped by the log limit and recorded in the replay turn by turn.

use ibc_engine::Ruleset;
use ibc_replay::Record;
use ibc_runner::{run_match, BotSpec, MatchConfig, MatchOutput};

const MAP: &str = include_str!("../../../maps/crossfire.ibcmap");
const TICKS: u32 = 6;

/// Ends its turn immediately, forever.
const IDLE: &str = r#"(module
  (import "ibc" "end_turn" (func $end_turn))
  (memory (export "memory") 1)
  (func (export "_start") (loop $l (call $end_turn) (br $l))))"#;

/// Every turn: "out\n" to stdout, "err\n" to stderr, "log" through `ibc.log` (which adds the newline), then
/// "two iovecs\n" to stdout in two pieces. Traps unless each `fd_write` succeeds and reports every byte.
const PRINTER: &str = r#"(module
  (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (import "ibc" "log" (func $log (param i32 i32) (result i32)))
  (import "ibc" "end_turn" (func $end_turn))
  (memory (export "memory") 1)
  (data (i32.const 0) "out\n")
  (data (i32.const 8) "err\n")
  (data (i32.const 16) "log")
  (data (i32.const 24) "two ")
  (data (i32.const 32) "iovecs\n")
  (data (i32.const 64) "\00\00\00\00\04\00\00\00")                  ;; { 0, 4 }
  (data (i32.const 72) "\08\00\00\00\04\00\00\00")                  ;; { 8, 4 }
  (data (i32.const 80) "\18\00\00\00\04\00\00\00\20\00\00\00\07\00\00\00")  ;; { 24, 4 } { 32, 7 }
  (func $write (param $fd i32) (param $iovs i32) (param $n i32) (param $len i32)
    (if (call $fd_write (local.get $fd) (local.get $iovs) (local.get $n) (i32.const 128)) (then (unreachable)))
    (if (i32.ne (i32.load (i32.const 128)) (local.get $len)) (then (unreachable))))
  (func (export "_start")
    (loop $l
      (call $write (i32.const 1) (i32.const 64) (i32.const 1) (i32.const 4))
      (call $write (i32.const 2) (i32.const 72) (i32.const 1) (i32.const 4))
      (if (call $log (i32.const 16) (i32.const 3)) (then (unreachable)))
      (call $write (i32.const 1) (i32.const 80) (i32.const 2) (i32.const 11))
      (call $end_turn)
      (br $l))))"#;
const TURN: &[u8] = b"out\nerr\nlog\ntwo iovecs\n";

/// Every turn: writes that must fail with EBADF (fds 0, 3 and -1) or EFAULT (iovecs outside memory) and
/// leave the byte count alone, then "ok\n" to stdout. Traps on any other answer.
const BAD_WRITES: &str = r#"(module
  (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (import "ibc" "end_turn" (func $end_turn))
  (memory (export "memory") 1)
  (data (i32.const 0) "ok\n")
  (data (i32.const 64) "\00\00\00\00\03\00\00\00")  ;; { 0, 3 }
  (data (i32.const 72) "\fa\ff\00\00\10\00\00\00")  ;; { 65530, 16 }: runs past the end of memory
  (func $fails (param $fd i32) (param $iovs i32) (param $errno i32)
    (i32.store (i32.const 128) (i32.const 77))
    (if (i32.ne (call $fd_write (local.get $fd) (local.get $iovs) (i32.const 1) (i32.const 128)) (local.get $errno))
      (then (unreachable)))
    (if (i32.ne (i32.load (i32.const 128)) (i32.const 77)) (then (unreachable))))
  (func (export "_start")
    (loop $l
      (call $fails (i32.const 0) (i32.const 64) (i32.const 8))
      (call $fails (i32.const 3) (i32.const 64) (i32.const 8))
      (call $fails (i32.const -1) (i32.const 64) (i32.const 8))
      (call $fails (i32.const 1) (i32.const 65536) (i32.const 21))
      (call $fails (i32.const 2) (i32.const 72) (i32.const 21))
      (if (call $fd_write (i32.const 1) (i32.const 64) (i32.const 1) (i32.const 128)) (then (unreachable)))
      (call $end_turn)
      (br $l))))"#;

fn bot(name: &str, wat: &str) -> BotSpec {
    BotSpec {
        name: name.into(),
        wasm: wat::parse_str(wat).unwrap(),
        fuel_multiplier: 1.0,
    }
}

fn run(wat: &str, log_limit: usize) -> MatchOutput {
    let out = run_match(MatchConfig {
        rules: Ruleset::builtin(),
        map_text: MAP.into(),
        seed: 3,
        bots: [bot("a", wat), bot("idle", IDLE)],
        log_limit,
        max_ticks: Some(TICKS),
        frames: false,
    })
    .unwrap();
    assert!(out.summary.faults.is_empty(), "{:?}", out.summary.faults);
    out
}

/// Team A's units, each with the log texts the replay recorded for it, one entry per turn that logged.
fn recorded_logs(out: &MatchOutput) -> Vec<(usize, Vec<Vec<u8>>)> {
    let team_a = (0..out.units.len()).filter(|&u| out.units[u].team == 0);
    team_a
        .map(|unit| {
            let texts = out
                .replay
                .ticks
                .iter()
                .flat_map(|t| &t.private[0])
                .filter_map(|r| match r {
                    Record::Log { unit: u, text } if *u as usize == unit => Some(text.clone()),
                    _ => None,
                });
            (unit, texts.collect())
        })
        .collect()
}

#[test]
fn stdout_stderr_and_ibc_log_share_the_unit_log_in_order() {
    let out = run(PRINTER, 1 << 20);
    let units = recorded_logs(&out);
    assert!(!units.is_empty());
    for (unit, turns) in units {
        assert_eq!(out.logs[unit], TURN.repeat(TICKS as usize), "unit {unit}");
        assert_eq!(
            turns,
            vec![TURN.to_vec(); TICKS as usize],
            "unit {unit}: one record per turn"
        );
    }
}

#[test]
fn the_cap_truncates_with_a_notice_and_writes_still_succeed() {
    const CAP: usize = 50;
    let out = run(PRINTER, CAP);
    for (unit, turns) in recorded_logs(&out) {
        let mut want = TURN.repeat(3)[..CAP].to_vec();
        want.extend_from_slice(format!("\n[ibc] log truncated at {CAP} bytes\n").as_bytes());
        assert_eq!(
            String::from_utf8_lossy(&out.logs[unit]),
            String::from_utf8_lossy(&want),
            "unit {unit}"
        );
        // The replay keeps exactly the bytes under the cap, without the notice.
        assert_eq!(
            turns.concat(),
            TURN.repeat(3)[..CAP].to_vec(),
            "unit {unit}"
        );
        assert_eq!(
            out.units[unit].turns, TICKS as u64,
            "the bot kept printing and was never faulted"
        );
    }
}

#[test]
fn writes_to_unknown_fds_or_bad_memory_fail_cleanly() {
    let out = run(BAD_WRITES, 1 << 20);
    for (unit, turns) in recorded_logs(&out) {
        assert_eq!(
            out.logs[unit],
            b"ok\n".repeat(TICKS as usize),
            "unit {unit}: nothing from the failed writes"
        );
        assert_eq!(turns.len(), TICKS as usize);
    }
}
