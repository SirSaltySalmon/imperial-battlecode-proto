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

/// Most fuel the instructions around a measured call may add to its fee in `fee_checker`.
const CALL: u64 = 24;

/// Every turn, measured with `fuel_remaining` around each call: `fd_write` of 100 one-byte iovecs (64 honoured
/// and charged), `random_get` of 4096 bytes and of 4097 (refused, and charged as 4096), and `ibc.log`, which
/// is free. Traps unless each call cost its fee plus at most `CALL` fuel of instructions.
fn fee_checker() -> String {
    let iovs = "\\00\\00\\00\\00\\01\\00\\00\\00".repeat(100); // { 0, 1 } x 100
    let write = 64 * ibc_abi::FUEL_IOV;
    let random = 4096 / 8 * ibc_abi::FUEL_RANDOM;
    format!(
        r#"(module
  (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "random_get" (func $random (param i32 i32) (result i32)))
  (import "ibc" "log" (func $log (param i32 i32) (result i32)))
  (import "ibc" "fuel_remaining" (func $fuel (result i64)))
  (import "ibc" "end_turn" (func $end_turn))
  (memory (export "memory") 1)
  (data (i32.const 0) "o")
  (data (i32.const 1024) "{iovs}")
  (func $paid (param $before i64) (param $fee i64) (local $cost i64)
    (local.set $cost (i64.sub (local.get $before) (call $fuel)))
    (if (i64.lt_s (local.get $cost) (local.get $fee)) (then (unreachable)))
    (if (i64.gt_s (local.get $cost) (i64.add (local.get $fee) (i64.const {CALL}))) (then (unreachable))))
  (func (export "_start") (local $f i64)
    (loop $l
      (local.set $f (call $fuel))
      (if (call $fd_write (i32.const 1) (i32.const 1024) (i32.const 100) (i32.const 128)) (then (unreachable)))
      (call $paid (local.get $f) (i64.const {write}))
      (local.set $f (call $fuel))
      (if (call $random (i32.const 4096) (i32.const 4096)) (then (unreachable)))
      (call $paid (local.get $f) (i64.const {random}))
      (local.set $f (call $fuel))
      (if (i32.ne (call $random (i32.const 4096) (i32.const 4097)) (i32.const 28)) (then (unreachable)))
      (call $paid (local.get $f) (i64.const {random}))
      (local.set $f (call $fuel))
      (if (call $log (i32.const 0) (i32.const 1)) (then (unreachable)))
      (call $paid (local.get $f) (i64.const 0))
      (call $end_turn)
      (br $l))))"#
    )
}

fn bot(name: &str, wat: &str) -> BotSpec {
    BotSpec::new(name, wat::parse_str(wat).unwrap())
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
fn fd_write_and_random_get_pay_by_size_and_log_is_free() {
    let out = run(&fee_checker(), 1 << 20);
    let units = recorded_logs(&out);
    assert!(!units.is_empty());
    for (unit, _) in units {
        assert_eq!(out.units[unit].turns, TICKS as u64);
        // 64 bytes from the iovecs honoured, "o\n" from the log call, every turn
        assert_eq!(
            out.logs[unit],
            [b"o".repeat(64), b"o\n".to_vec()]
                .concat()
                .repeat(TICKS as usize)
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
