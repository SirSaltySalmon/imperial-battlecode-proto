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
    let memory = rules.memory_bytes as usize;
    let game = Arc::new(Mutex::new(Match::new(rules, map, 1)));
    UnitVm::new(&env.engine, &env.linker, &module, game, 0, 1, 1 << 10, memory).unwrap()
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
