//! Recursion depth: why it must be counted in the module, not by the native stack.
//!
//! Each test bot logs one byte per level and recurses until it traps, so the length of its log is the
//! depth it reached. Wasmtime by itself traps when the native stack runs out, and a frame's native size is
//! decided by Cranelift for the CPU it compiles for, so that depth is not a function of the module.

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
    let mut per_unit = Vec::new();
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
        per_unit.push((frame, unit));
    }
    let depth = |f| native_depth(f, 512 << 10);
    assert!(depth(Frame::Thin) > depth(Frame::I64(16)) && depth(Frame::I64(16)) > depth(Frame::I64(64)));
    // Same logical frame, different native frame: v128 slots are twice as wide as i64 slots.
    assert!(depth(Frame::V128(64)) < depth(Frame::I64(64)), "v128 frames are larger natively");
}
