//! Recursion depth: why it must be counted in the module, not by the native stack.
//!
//! Each test bot logs one byte per level and recurses until it traps, so the length of its log is the
//! depth it reached. Wasmtime by itself traps when the native stack runs out, and a frame's native size is
//! decided by Cranelift for the CPU it compiles for, so that depth is not a function of the module. The
//! runner counts the stack in stack units instead (`src/stack.rs`).

use ibc_engine::Ruleset;
use ibc_runner::{run_match, BotSpec, MatchConfig, MatchOutput, MAX_STACK_UNITS};
use wasmtime::{Caller, Config, Engine, Linker, Module, Store, Trap};

/// What each level of the recursion keeps in its frame across the recursive call.
#[derive(Clone, Copy, Debug)]
enum Frame {
    /// Nothing but the call.
    Thin,
    /// `n` i64 locals.
    I64(u32),
    /// `n` v128 locals.
    V128(u32),
    /// `n` i64 values on the operand stack.
    Operands(u32),
}

impl Frame {
    /// The cost of one level in stack units: params + locals + maximum operand stack height + 2 per frame
    /// (`docs/RULES.md`, "Compute and memory"). Every variant needs two operands for the `log` call.
    fn units(self) -> u32 {
        match self {
            Frame::Thin => 2 + 2,
            Frame::I64(n) | Frame::V128(n) => n + 2 + 2,
            Frame::Operands(n) => n.max(2) + 2,
        }
    }
}

/// A bot whose `_start` calls `$f`, which logs one byte and calls itself with `frame` kept live across
/// the call (it is used after the call, which never returns). The values are loaded from memory so that
/// Cranelift cannot recompute them after the call instead of keeping them.
fn recurser(frame: Frame) -> String {
    let (mut locals, mut before, mut after) = (String::new(), String::new(), String::new());
    match frame {
        Frame::Thin => {}
        Frame::I64(n) => {
            for i in 0..n {
                locals += &format!("(local $l{i} i64)");
                before += &format!("(local.set $l{i} (i64.load offset={} (i32.const 16)))", 8 * i);
                after += &format!("(global.set $g (i64.add (global.get $g) (local.get $l{i})))");
            }
        }
        Frame::V128(n) => {
            for i in 0..n {
                locals += &format!("(local $v{i} v128)");
                before += &format!("(local.set $v{i} (v128.load offset={} (i32.const 16)))", 16 * i);
                after += &format!("(global.set $g (i64.add (global.get $g) (i64x2.extract_lane 0 (local.get $v{i}))))");
            }
        }
        Frame::Operands(n) => {
            for i in 0..n {
                before += &format!("(i64.load offset={} (i32.const 16))", 8 * i);
            }
            after = format!("{}(global.set $g)", "(i64.add)".repeat(n.saturating_sub(1) as usize));
            if n == 0 {
                after.clear();
            }
        }
    }
    format!(
        r#"(module
  (import "ibc" "log" (func $log (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "\n")
  (global $g (mut i64) (i64.const 0))
  (func $f {locals}
    (drop (call $log (i32.const 0) (i32.const 1)))
    {before}
    (call $f)
    {after})
  (func (export "_start") (call $f)))"#
    )
}

/// Depth reached by `frame` in plain Wasmtime (no instrumentation) with `max_wasm_stack` bytes of stack.
fn native_depth(frame: Frame, max_wasm_stack: usize) -> u32 {
    let mut config = Config::new();
    config.max_wasm_stack(max_wasm_stack);
    let engine = Engine::new(&config).unwrap();
    let module = Module::new(&engine, wat::parse_str(recurser(frame)).unwrap()).unwrap();
    let mut store = Store::new(&engine, 0u32);
    let mut linker = Linker::new(&engine);
    linker
        .func_wrap("ibc", "log", |mut c: Caller<'_, u32>, _ptr: i32, _len: i32| -> i32 {
            *c.data_mut() += 1;
            0
        })
        .unwrap();
    let instance = linker.instantiate(&mut store, &module).unwrap();
    let start = instance.get_typed_func::<(), ()>(&mut store, "_start").unwrap();
    let err = start.call(&mut store, ()).unwrap_err();
    assert_eq!(err.downcast_ref::<Trap>(), Some(&Trap::StackOverflow), "{frame:?}: {err:?}");
    *store.data()
}

const FRAMES: [Frame; 6] =
    [Frame::Thin, Frame::I64(16), Frame::I64(64), Frame::V128(16), Frame::V128(64), Frame::Operands(64)];

/// The evidence: without a deterministic limit the fault depth is the native stack divided by the native
/// frame size. Both are outside the module: the frame size is Cranelift's per-ISA layout, the stack size is
/// host configuration. On x64 the same module reaches a different depth for every stack size, and frames of
/// equal logical size (i64 vs v128 locals) reach different depths because their native slots differ.
#[test]
fn native_fault_depth_depends_on_frame_layout_and_stack_size() {
    eprintln!("frame          units  depth@256K  depth@512K  bytes/frame  bytes/unit");
    for frame in FRAMES {
        let small = native_depth(frame, 256 << 10);
        let large = native_depth(frame, 512 << 10);
        let bytes = (512u64 << 10) as f64 / large as f64;
        let unit = bytes / frame.units() as f64;
        eprintln!(
            "{:<14} {:>5}  {small:>10}  {large:>10}  {bytes:>11.1}  {unit:>10.1}",
            format!("{frame:?}"),
            frame.units()
        );
        // Twice the native stack, twice the depth (to within the frames below the first call).
        let ratio = large as f64 / small as f64;
        assert!((1.95..=2.05).contains(&ratio), "{frame:?}: {small} -> {large}");
    }
    let depth = |f| native_depth(f, 512 << 10);
    assert!(depth(Frame::Thin) > depth(Frame::I64(16)) && depth(Frame::I64(16)) > depth(Frame::I64(64)));
    // Same logical frame, different native frame: v128 slots are twice as wide as i64 slots.
    assert!(depth(Frame::V128(64)) < depth(Frame::I64(64)), "v128 frames are larger natively");
}

/// `_start` only calls `$f`: no params, locals or operands, so 2 units.
const START_UNITS: u32 = 2;

fn run_recurser(frame: Frame, stack_limit: i32) -> MatchOutput {
    let mut rules = Ruleset::builtin();
    rules.stack_limit = stack_limit;
    let idle = r#"(module
      (import "ibc" "end_turn" (func $end_turn))
      (memory (export "memory") 1)
      (func (export "_start") (loop $l (call $end_turn) (br $l))))"#;
    let bot =
        |name: &str, wat: &str| BotSpec::new(name, wat::parse_str(wat).unwrap());
    run_match(MatchConfig {
        rules,
        map_text: include_str!("../../../maps/crossfire.ibcmap").into(),
        seed: 1,
        bots: [bot("recurser", &recurser(frame)), bot("idle", idle)],
        log_limit: 8 << 20,
        max_ticks: Some(10),
        frames: false,
    })
    .unwrap()
}

/// For each unit of team A, which must all have faulted: how many levels it logged, and the rest of its log.
fn levels(out: &MatchOutput) -> Vec<(u32, String)> {
    let team_a = out.units.iter().enumerate().filter(|(_, u)| u.team == 0);
    team_a
        .map(|(id, u)| {
            assert!(u.faulted, "unit {id} did not fault");
            let n = out.logs[id].iter().take_while(|&&b| b == b'\n').count();
            (n as u32, String::from_utf8_lossy(&out.logs[id][n..]).into_owned())
        })
        .collect()
}

/// The fix: the runner counts the stack in the module, so the depth is the limit divided by the frame's
/// stack units, whatever Cranelift makes of the frame.
#[test]
fn the_depth_is_the_limit_over_the_frame_cost() {
    let limit = Ruleset::builtin().stack_limit;
    for frame in FRAMES {
        let out = run_recurser(frame, limit);
        let expected = (limit as u32 - START_UNITS) / frame.units();
        for (depth, rest) in levels(&out) {
            assert_eq!(depth, expected, "{frame:?}");
            assert!(
                rest.contains(&format!("stack limit exceeded (deeper than stack_limit = {limit} units)")),
                "{rest}"
            );
        }
        // The fault is a replay input like any other.
        ibc_replay::verify(&out.replay).unwrap();
    }
}

/// The margin: at the largest limit the runner accepts, the deterministic limit still comes first, also for
/// frames whose native slots are widest (v128).
#[test]
fn the_limit_comes_before_the_native_stack() {
    for frame in FRAMES {
        let out = run_recurser(frame, MAX_STACK_UNITS as i32);
        let expected = (MAX_STACK_UNITS - START_UNITS) / frame.units();
        for (depth, rest) in levels(&out) {
            assert_eq!(depth, expected, "{frame:?}: {rest}");
            assert!(rest.contains("stack limit exceeded"), "{frame:?}: {rest}");
        }
    }
}

#[test]
fn a_limit_above_what_the_runner_supports_is_refused() {
    let mut rules = Ruleset::builtin();
    rules.stack_limit = MAX_STACK_UNITS as i32 + 1;
    let wasm = wat::parse_str(recurser(Frame::Thin)).unwrap();
    let bot = || BotSpec::new("recurser", wasm.clone());
    let cfg = MatchConfig {
        rules,
        map_text: include_str!("../../../maps/crossfire.ibcmap").into(),
        seed: 1,
        bots: [bot(), bot()],
        log_limit: 1024,
        max_ticks: Some(1),
        frames: false,
    };
    let err = run_match(cfg).err().expect("refused").to_string();
    assert!(err.contains("stack_limit"), "{err}");
}
