//! Pins the Wasmtime behaviour behind `UnitVm::store_mut`: fuel read and re-armed between polls of a
//! suspended `_start` call is exact, per turn. A Wasmtime bump that breaks it fails here, at the turn
//! where the budget was not kept, instead of silently miscounting fuel in matches.

use super::*;
use ibc_engine::{Map, Ruleset};

/// Longest check-free path of the bots below, in fuel (about 9; the margin allows for operator costs).
const PATH: u64 = 16;

/// Counts in pure wasm forever: every turn ends in Wasmtime's own out-of-fuel suspension.
const SPIN: &str = r#"(module
  (memory (export "memory") 1)
  (func (export "_start") (local $i i32)
    (loop $l
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $l))))"#;

/// Calls `end_turn`, then counts to 2000 (a burst of about 16,000 fuel), forever. A burst spans as many
/// turns as it needs, so turns end both in `end_turn` and in out-of-fuel suspensions.
const BURSTS: &str = r#"(module
  (import "ibc" "end_turn" (func $end_turn))
  (memory (export "memory") 1)
  (func (export "_start") (local $j i32)
    (loop $turn
      (call $end_turn)
      (local.set $j (i32.const 0))
      (loop $count
        (local.set $j (i32.add (local.get $j) (i32.const 1)))
        (br_if $count (i32.lt_u (local.get $j) (i32.const 2000))))
      (br $turn))))"#;

/// One `debug_draw` per turn, forever.
const DRAWER: &str = r#"(module
  (import "ibc" "end_turn" (func $end_turn))
  (import "ibc" "debug_draw" (func $draw (param i32 i32 i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "_start")
    (loop $l
      (drop (call $draw (i32.const 2) (i32.const 0) (i32.const 0) (i32.const 0) (i32.const 0) (i32.const 0)))
      (call $end_turn)
      (br $l))))"#;

/// One `random_get` of 4096 bytes per turn, forever.
const RANDOMS: &str = r#"(module
  (import "ibc" "end_turn" (func $end_turn))
  (import "wasi_snapshot_preview1" "random_get" (func $random (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "_start")
    (loop $l
      (drop (call $random (i32.const 0) (i32.const 4096)))
      (call $end_turn)
      (br $l))))"#;

/// Budgets that shrink and grow by large factors, some below one check-free path (paid from debt), so a
/// slice that kept its old size or an odometer that missed consumption shows at once.
const BUDGETS: &[u64] = &[100_000, 1_000, 250_000, 40_000, 3, 7, 60_000, 1, 2_000_000, 500, 30_000, 10, 900, 5];

const BROKEN: &str = "fuel read or re-armed between polls is no longer exact: re-check the SAFETY argument of \
                      `UnitVm::store_mut` against this Wasmtime";

fn unit_vm(wat: &str) -> UnitVm {
    let env = crate::host().unwrap();
    let module = Module::new(&env.engine, wat::parse_str(wat).unwrap()).unwrap();
    let map = Map::parse(include_str!("../../../../maps/crossfire.ibcmap")).unwrap();
    let rules = Ruleset::builtin();
    let (memory, stack) = (rules.memory_bytes as usize, rules.stack_limit as u32);
    let game = Arc::new(Mutex::new(Match::new(rules, map, 1)));
    UnitVm::new(&env.engine, &env.linker, &module, game, 0, 1, 1 << 10, memory, stack).unwrap()
}

/// Runs one turn per budget and checks each against the fuel model (module docs): a turn runs at most one
/// check-free path past its allowance, a turn that ran out of fuel used at least its allowance, and the
/// overshoot is the debt the next turn starts with. Returns the fuel used and how each turn ended.
fn run_checked(vm: &mut UnitVm, budgets: &[u64]) -> Vec<(u64, bool)> {
    let mut debt = 0;
    let mut turns = Vec::new();
    for (i, &budget) in budgets.iter().enumerate() {
        let turn = vm.run_turn(budget);
        let yielded = match turn.end {
            TurnEnd::Yielded => true,
            TurnEnd::OutOfFuel => false,
            TurnEnd::Fault(e) => panic!("turn {i}: {e}"),
        };
        if debt >= budget {
            assert!(turn.used == 0 && !yielded, "turn {i}: ran on a budget its debt of {debt} covers");
            debt -= budget;
        } else {
            let allowance = budget - debt;
            let kept = turn.used < allowance + PATH && (yielded || turn.used >= allowance);
            assert!(kept, "turn {i}: used {} of an allowance of {allowance}: {BROKEN}", turn.used);
            debt = turn.used.saturating_sub(allowance);
        }
        turns.push((turn.used, yielded));
    }
    turns
}

#[test]
fn out_of_fuel_suspensions_keep_every_budget() {
    let mut vm = unit_vm(SPIN);
    let turns = run_checked(&mut vm, BUDGETS);
    assert!(turns.iter().all(|&(_, yielded)| !yielded));
    // Over the whole run the bot used its budgets plus less than one path: nothing lost, nothing extra.
    let (used, granted) = (turns.iter().map(|t| t.0).sum::<u64>(), BUDGETS.iter().sum::<u64>());
    assert!(used >= granted && used < granted + PATH, "used {used} of {granted}");
}

#[test]
fn end_turn_and_out_of_fuel_suspensions_mix() {
    let mut vm = unit_vm(BURSTS);
    let turns = run_checked(&mut vm, BUDGETS);
    // Every burst costs the same fuel however many turns it spans, so the odometer read at each
    // suspension adds up to exactly what the bot executed. The first turn only reaches `end_turn`.
    let mut bursts = Vec::new();
    let mut sum = 0;
    let mut spans = 0;
    for &(used, yielded) in &turns[1..] {
        sum += used;
        spans += 1;
        if yielded {
            bursts.push((sum, spans));
            (sum, spans) = (0, 0);
        }
    }
    assert!(bursts.len() >= 4, "{turns:?}");
    assert!(bursts.iter().any(|b| b.1 == 1) && bursts.iter().any(|b| b.1 > 2), "{bursts:?}");
    assert!(bursts.iter().all(|b| b.0 == bursts[0].0), "bursts cost different fuel: {bursts:?}");
}

#[test]
fn a_fee_above_every_budget_delays_the_call_and_never_blocks_it() {
    let mut vm = unit_vm(DRAWER);
    let (budget, turns) = (ibc_abi::FUEL_DEBUG / 3, 60);
    let (mut used, mut drawn) = (0, 0);
    for _ in 0..turns {
        let turn = vm.run_turn(budget);
        assert!(!matches!(turn.end, TurnEnd::Fault(_)));
        used += turn.used;
        drawn += lock(&vm.store_mut().data().game).take_debug().len() as u64;
    }
    // Each drawing takes what is left of its turn and pays the rest over the turns after.
    assert!(drawn >= turns / 8, "only {drawn} drawings in {turns} turns");
    // Every drawing paid its fee in full, and at most one fee is still owed at the end.
    assert!(
        used >= drawn * ibc_abi::FUEL_DEBUG,
        "{drawn} drawings for {used} fuel"
    );
    assert!(
        used <= turns * budget + ibc_abi::FUEL_DEBUG + PATH,
        "used {used}"
    );
}

#[test]
fn a_wasi_fee_is_paid_at_once_and_an_overrun_is_owed() {
    let fee = 4096 / 8 * ibc_abi::FUEL_RANDOM;
    let mut vm = unit_vm(RANDOMS);
    // On a budget that covers the fee, it is paid within the turn, which ends in `end_turn`.
    let first = vm.run_turn(100_000);
    assert!(
        matches!(first.end, TurnEnd::Yielded) && first.used >= fee && first.used < fee + PATH,
        "{}",
        first.used
    );
    // On a budget below the fee, the call goes through at once and its turn runs over by the rest of the fee,
    // which the next turns pay off before the bot runs again.
    let (budget, turns) = (fee / 5, 60);
    let (mut used, mut calls) = (0, 0);
    for _ in 0..turns {
        let rng = vm.store_mut().data().rng;
        let turn = vm.run_turn(budget);
        assert!(!matches!(turn.end, TurnEnd::Fault(_)));
        calls += u64::from(vm.store_mut().data().rng != rng);
        used += turn.used;
    }
    assert!(calls >= turns / 8, "only {calls} calls in {turns} turns");
    assert!(
        used >= calls * fee && used <= turns * budget + fee + PATH,
        "{calls} calls for {used} fuel"
    );
}
