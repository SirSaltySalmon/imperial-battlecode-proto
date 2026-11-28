//! Per-unit memory and table caps: the memory cap is the ruleset's `memory_bytes`, tables are capped by the
//! runner. Checked with tiny hand-written WAT bots.

use ibc_engine::Ruleset;
use ibc_runner::{run_match, BotSpec, MatchConfig, MatchOutput};

const MAP: &str = include_str!("../../../maps/crossfire.ibcmap");
const PAGE: i32 = 65_536;

/// Ends its turn immediately, forever, with `extra` added to the module (a memory, tables).
fn idle(extra: &str) -> String {
    format!(
        r#"(module
  (import "ibc" "end_turn" (func $end_turn))
  {extra}
  (func (export "_start") (loop $l (call $end_turn) (br $l))))"#
    )
}

/// Starts with 17 pages (like the reference bots), grows to exactly `pages`, then checks that one more page
/// is refused with -1. Traps if either result is not the expected one.
fn grower(pages: i32) -> String {
    format!(
        r#"(module
  (import "ibc" "end_turn" (func $end_turn))
  (memory (export "memory") 17)
  (func (export "_start")
    (if (i32.ne (memory.grow (i32.const {})) (i32.const 17)) (then unreachable))
    (if (i32.ne (memory.grow (i32.const 1)) (i32.const -1)) (then unreachable))
    (if (i32.ne (memory.size) (i32.const {pages})) (then unreachable))
    (loop $l (call $end_turn) (br $l))))"#,
        pages - 17
    )
}

fn bot(name: &str, wat: &str) -> BotSpec {
    BotSpec::new(name, wat::parse_str(wat).unwrap())
}

fn run(rules: Ruleset, a: BotSpec, b: BotSpec, ticks: u32) -> MatchOutput {
    run_match(MatchConfig {
        rules,
        map_text: MAP.into(),
        seed: 7,
        bots: [a, b],
        log_limit: 1 << 16,
        max_ticks: Some(ticks),
        frames: false,
    })
    .unwrap()
}

/// Every unit of team A faulted before its first turn with a reason containing `why`; team B none.
fn team_a_refused(out: &MatchOutput, why: &str) {
    assert!(!out.summary.faults.is_empty());
    for f in &out.summary.faults {
        assert_eq!((f.team, f.tick), (0, 0), "{f:?}");
        assert!(
            f.reason.starts_with("instantiation failed") && f.reason.contains(why),
            "{}",
            f.reason
        );
    }
    for u in &out.units {
        assert_eq!(u.faulted, u.team == 0);
        assert_eq!(u.turns == 0, u.team == 0);
    }
}

#[test]
fn memory_grows_to_the_rule_and_no_further() {
    let rules = Ruleset::builtin();
    assert_eq!(rules.memory_bytes, 8 << 20);
    let pages = rules.memory_bytes / PAGE;
    let out = run(rules, bot("a", &grower(pages)), bot("b", &grower(pages)), 3);
    assert!(out.summary.faults.is_empty(), "{:?}", out.summary.faults);

    // The cap comes from the match's ruleset: a smaller one stops growth earlier.
    let mut small = Ruleset::builtin();
    small.memory_bytes = 32 * PAGE;
    let out = run(small, bot("a", &grower(32)), bot("b", &grower(32)), 3);
    assert!(out.summary.faults.is_empty(), "{:?}", out.summary.faults);
}

#[test]
fn declaring_more_memory_than_the_rule_faults_at_instantiation() {
    let pages = Ruleset::builtin().memory_bytes / PAGE;
    let over = idle(&format!(r#"(memory (export "memory") {})"#, pages + 1));
    let at = idle(&format!(r#"(memory (export "memory") {pages})"#));
    let out = run(Ruleset::builtin(), bot("over", &over), bot("at", &at), 2);
    team_a_refused(&out, "exceeds memory limits");
}

#[test]
fn a_unit_gets_one_table_of_at_most_65536_elements() {
    let memory = r#"(memory (export "memory") 1)"#;
    let ok = idle(&format!("{memory} (table 65536 funcref)"));
    for (bad, why) in [
        (
            idle(&format!("{memory} (table 1 funcref) (table 1 funcref)")),
            "table count too high",
        ),
        (
            idle(&format!("{memory} (table 65537 funcref)")),
            "exceeds table limits",
        ),
    ] {
        let out = run(Ruleset::builtin(), bot("bad", &bad), bot("ok", &ok), 2);
        team_a_refused(&out, why);
    }
}

#[test]
fn a_huge_memory_copy_finishes_and_is_paid_for_in_later_turns() {
    const LEN: i32 = 3 << 20;
    // Copies 3 MiB in one instruction (1 fuel per byte: three draft-tick budgets), checks the copy, then
    // ends its turns.
    let copier = format!(
        r#"(module
  (import "ibc" "end_turn" (func $end_turn))
  (memory (export "memory") 100)
  (data (i32.const 0) "x")
  (func (export "_start")
    (memory.copy (i32.const {LEN}) (i32.const 0) (i32.const {LEN}))
    (if (i32.ne (i32.load8_u (i32.const {LEN})) (i32.const 120)) (then unreachable))
    (loop $l (call $end_turn) (br $l))))"#
    );
    let out = run(
        Ruleset::builtin(),
        bot("copier", &copier),
        bot("idle", &idle(r#"(memory (export "memory") 1)"#)),
        12,
    );
    assert!(out.summary.faults.is_empty(), "{:?}", out.summary.faults);
    for u in out.units.iter().filter(|u| u.team == 0) {
        assert!(
            u.max_turn >= LEN as u64,
            "the copy runs to completion in one turn: {}",
            u.max_turn
        );
        // That turn overruns its budget; the overrun is taken out of the next turns, which run nothing.
        assert!(u.out_of_fuel_turns >= 3, "{u:?}");
    }
}

/// A ruleset whose `python` row doubles the memory cap and triples fuel; every other language gets x1.
fn with_python_row() -> Ruleset {
    let mut rules = Ruleset::builtin();
    rules.languages.push(ibc_engine::LanguageRules { name: "python".into(), fuel_milli: 3000, memory_milli: 2000 });
    rules
}

#[test]
fn a_language_row_scales_the_memory_cap_and_fuel_of_its_team_only() {
    let pages = Ruleset::builtin().memory_bytes / PAGE;
    let mut a = bot("a", &grower(2 * pages));
    a.language = "python".into();
    // Team B is not python: it grows to the plain cap.
    let out = run(with_python_row(), a, bot("b", &grower(pages)), 3);
    assert!(out.summary.faults.is_empty(), "{:?}", out.summary.faults);
    let h = &out.replay.header.bots;
    assert_eq!((h[0].language.as_str(), h[0].fuel_multiplier, h[0].memory_multiplier), ("python", 3.0, 2.0));
    assert_eq!((h[1].language.as_str(), h[1].fuel_multiplier, h[1].memory_multiplier), ("default", 1.0, 1.0));
    assert_eq!(out.summary.teams[0].fuel.granted, 3 * out.summary.teams[1].fuel.granted);
    assert_eq!(out.summary.teams[0].language, "python");

    // The same module on team B, not python, cannot grow past the plain cap.
    let out = run(with_python_row(), bot("a", &grower(pages)), bot("b", &grower(2 * pages)), 3);
    assert!(out.summary.faults.iter().all(|f| f.team == 1) && !out.summary.faults.is_empty());
}
