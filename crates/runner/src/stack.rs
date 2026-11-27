//! Deterministic recursion limit: the depth at which a recursing bot traps must not depend on the CPU.
//!
//! Wasmtime traps when the native stack runs out, and the native size of a wasm frame is whatever Cranelift
//! lays out for the ISA it compiles for (and changes between Cranelift versions). Left alone, a bot that
//! recurses until it traps would fault at another depth, so at another fuel count or tick, on an arm64 worker
//! than on x64 (`tests/stack.rs` measures it). This is the stack height limiter of Parity's
//! `wasm-instrument` (`inject_stack_limiter`, as used by Substrate and Polkadot), done over wasmparser so
//! that it handles every feature Wasmtime accepts:
//!
//! * a function's frame costs `params + locals + maximum operand stack height + FRAME_UNITS` stack units,
//!   fixed per function; the operand stack height comes from wasmparser's validator;
//! * a mutable global added to the module holds the stack height of the running function. A function adds
//!   its cost on entry, keeps the sum in a local added to it, and traps if the sum exceeds the limit;
//! * a caller puts its own height back after every call returns and on entry to a catch handler, so the
//!   callee needs no bookkeeping on any of its exits (its end, `return`, a branch, an exception); a tail call
//!   takes the caller's frame off before it jumps.
//!
//! wasm-instrument charges a callee's cost at each call site, and so needs thunks for indirect calls, table
//! entries and exports; charging on entry covers those, and `call_ref`, without them. The runner sizes the
//! native stack so that the logical limit always triggers first (`NATIVE_BYTES_PER_UNIT` in `lib.rs`).

use wasm_encoder::{BlockType, ConstExpr, Encode, ExportKind, GlobalType, InstructionSink, RawSection, ValType};
use wasmparser::{
    BinaryReader, Catch, FuncValidator, FunctionBody, Operator, OperatorsReader, Parser, Payload, ValidPayload,
    Validator, ValidatorResources, WasmFeatures,
};

/// Export through which the runner reads the stack height after a trap.
pub(crate) const HEIGHT_EXPORT: &str = "__ibc_stack_height";
/// Units a frame costs besides its values: return address and frame pointer. wasm-instrument's
/// `ACTIVATION_FRAME_COST`; it also makes a function with no values cost something.
pub(crate) const FRAME_UNITS: u32 = 2;

/// Non-custom section ids in the order the binary format requires.
const ORDER: [u8; 13] = [1, 2, 3, 4, 5, 13, 6, 7, 8, 9, 12, 10, 11];
const GLOBAL_SECTION: u8 = 6;
const EXPORT_SECTION: u8 = 7;
const CODE_SECTION: u8 = 10;

fn rank(id: u8) -> usize {
    ORDER.iter().position(|&s| s == id).unwrap_or(ORDER.len())
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn at(offset: u64) -> usize {
    usize::try_from(offset).expect("offsets are in memory")
}

/// `wasm` with every function charged against a stack of `limit` units. `wasm` must be a valid module.
pub(crate) fn instrument(wasm: &[u8], limit: u32) -> Result<Vec<u8>, String> {
    i32::try_from(limit).map_err(|_| format!("stack limit {limit} is too large"))?;
    let mut parser = Parser::new(0);
    parser.set_features(WasmFeatures::all());
    let mut validator = Validator::new_with_features(WasmFeatures::all());
    let mut w = Writer {
        wasm,
        limit,
        out: wasm_encoder::Module::new(),
        imported_funcs: 0,
        globals: 0,
        height_global: None,
        exported: false,
        code: wasm_encoder::CodeSection::new(),
        code_left: 0,
    };
    for payload in parser.parse_all(wasm) {
        let payload = payload.map_err(err)?;
        let valid = validator.payload(&payload).map_err(err)?;
        if let Some(types) = validator.types(0) {
            w.globals = types.global_count();
            if let Payload::ImportSection(_) = payload {
                w.imported_funcs = types.function_count();
            }
        }
        match (payload, valid) {
            (Payload::Version { encoding, .. }, _) if encoding != wasmparser::Encoding::Module => {
                return Err("not a core module".into());
            }
            (Payload::CodeSectionEntry(body), ValidPayload::Func(func, _)) => {
                let body = w.function(func.into_validator(Default::default()), &body)?;
                w.code.raw(&body);
                w.code_left -= 1;
                if w.code_left == 0 {
                    w.out.section(&w.code);
                }
            }
            (payload, _) => w.section(&payload)?,
        }
    }
    Ok(w.out.finish())
}

struct Writer<'a> {
    wasm: &'a [u8],
    limit: u32,
    out: wasm_encoder::Module,
    imported_funcs: u32,
    /// Globals the module imports and defines, as far as it has been read.
    globals: u32,
    /// Index of the height global, once its section is written.
    height_global: Option<u32>,
    exported: bool,
    code: wasm_encoder::CodeSection,
    code_left: u32,
}

impl Writer<'_> {
    /// Copy a section, adding the height global and its export (in sections of their own if the module has
    /// none).
    fn section(&mut self, payload: &Payload<'_>) -> Result<(), String> {
        let (id, range) = match payload {
            Payload::End(_) => return self.before(ORDER.len()),
            Payload::CodeSectionStart { count, .. } => {
                self.before(rank(CODE_SECTION))?;
                self.code_left = *count;
                if *count == 0 {
                    self.out.section(&self.code);
                }
                return Ok(());
            }
            p => match p.as_section() {
                Some(s) => s,
                None => return Ok(()),
            },
        };
        let data = &self.wasm[at(range.start)..at(range.end)];
        match payload {
            Payload::GlobalSection(_) => return self.globals(data),
            Payload::ExportSection(exports) => {
                for e in exports.clone() {
                    if e.map_err(err)?.name == HEIGHT_EXPORT {
                        return Err(format!("the export name `{HEIGHT_EXPORT}` is reserved"));
                    }
                }
                self.before(rank(EXPORT_SECTION))?;
                return self.exports(data);
            }
            _ => {}
        }
        if id != 0 {
            self.before(rank(id))?;
        }
        self.out.section(&RawSection { id, data });
        Ok(())
    }

    /// Write the global and export sections now if the next section must come after them.
    fn before(&mut self, next: usize) -> Result<(), String> {
        if next > rank(GLOBAL_SECTION) && self.height_global.is_none() {
            self.globals(&[0])?;
        }
        if next > rank(EXPORT_SECTION) && !self.exported {
            self.exports(&[0])?;
        }
        Ok(())
    }

    /// The global section `data` (count, then entries) with the height global appended after every
    /// global imported or defined so far.
    fn globals(&mut self, data: &[u8]) -> Result<(), String> {
        let mut entry = Vec::new();
        GlobalType { val_type: ValType::I32, mutable: true, shared: false }.encode(&mut entry);
        ConstExpr::i32_const(0).encode(&mut entry);
        self.out.section(&RawSection { id: GLOBAL_SECTION, data: &append(data, &entry)? });
        self.height_global = Some(self.globals);
        Ok(())
    }

    /// The export section `data` with the height global exported as `HEIGHT_EXPORT`.
    fn exports(&mut self, data: &[u8]) -> Result<(), String> {
        let index = self.height_global.ok_or("exports before globals")?;
        let mut entry = Vec::new();
        HEIGHT_EXPORT.encode(&mut entry);
        ExportKind::Global.encode(&mut entry);
        index.encode(&mut entry);
        self.out.section(&RawSection { id: EXPORT_SECTION, data: &append(data, &entry)? });
        self.exported = true;
        Ok(())
    }

    /// One function body, re-encoded with its stack bookkeeping. Operators are copied byte for byte.
    fn function(&self, mut fv: FuncValidator<ValidatorResources>, body: &FunctionBody<'_>) -> Result<Vec<u8>, String> {
        let global = self.height_global.ok_or("code before globals")?;
        let mut reader = body.get_binary_reader();
        let mut groups_reader = reader.clone();
        let groups = groups_reader.read_var_u32().map_err(err)?;
        let groups_start = at(groups_reader.original_position());
        fv.read_locals(&mut reader).map_err(err)?;
        let ops_start = reader.original_position();
        // The new local comes after every existing one, so no local index changes.
        let local = fv.len_locals();
        let scan = scan(&mut fv, OperatorsReader::new(reader), self.imported_funcs)?;
        let cost = local
            .checked_add(scan.max_height)
            .and_then(|c| c.checked_add(FRAME_UNITS))
            .and_then(|c| i32::try_from(c).ok())
            .ok_or("function frame too large")?;

        let mut out = Vec::new();
        (groups + 1).encode(&mut out);
        out.extend_from_slice(&self.wasm[groups_start..at(ops_start)]);
        1u32.encode(&mut out);
        ValType::I32.encode(&mut out);
        InstructionSink::new(&mut out)
            .global_get(global)
            .i32_const(cost)
            .i32_add()
            .local_tee(local)
            .global_set(global)
            .local_get(local)
            .i32_const(self.limit as i32)
            .i32_gt_u()
            .if_(BlockType::Empty)
            .unreachable()
            .end();
        let mut cursor = at(ops_start);
        for (offset, mark) in scan.marks {
            out.extend_from_slice(&self.wasm[cursor..at(offset)]);
            let mut sink = InstructionSink::new(&mut out);
            match mark {
                Mark::Restore => sink.local_get(local).global_set(global),
                Mark::TailCall => sink.local_get(local).i32_const(cost).i32_sub().global_set(global),
            };
            cursor = at(offset);
        }
        out.extend_from_slice(&self.wasm[cursor..at(body.range().end)]);
        Ok(out)
    }
}

/// `section` (an entry count, then the entries) with one more entry, `entry`, at the end.
fn append(section: &[u8], entry: &[u8]) -> Result<Vec<u8>, String> {
    let mut reader = BinaryReader::new(section, 0);
    let count = reader.read_var_u32().map_err(err)?;
    let mut out = Vec::with_capacity(section.len() + entry.len() + 5);
    (count + 1).encode(&mut out);
    out.extend_from_slice(&section[reader.current_position()..]);
    out.extend_from_slice(entry);
    Ok(out)
}

/// Bookkeeping at one offset. Where both fall on one offset (a tail call right after a call), the tail
/// call's sorts first and is the one kept: it sets the height that the next frame needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Mark {
    /// Take this function's frame off: before a tail call.
    TailCall,
    /// Set the height back to this function's: after a call returns, on entry to a catch handler.
    Restore,
}

struct Scan {
    max_height: u32,
    /// Byte offsets in the original module, sorted, at which bookkeeping goes in.
    marks: Vec<(u64, Mark)>,
}

/// A label of the function, as far as catch handlers are concerned.
struct Label {
    /// Offset of the first operator of a loop's body (a branch to a loop lands there).
    loop_body: Option<u64>,
    /// A catch clause branches to this label.
    caught: bool,
}

/// Validate a function's operators, measuring the operand stack and finding where bookkeeping goes.
fn scan(
    fv: &mut FuncValidator<ValidatorResources>,
    mut ops: OperatorsReader<'_>,
    imported_funcs: u32,
) -> Result<Scan, String> {
    let mut s = Scan { max_height: 0, marks: Vec::new() };
    // The function body itself: a catch that branches there returns, and the caller restores.
    let mut labels = vec![Label { loop_body: None, caught: false }];
    while !ops.eof() {
        let (op, offset) = ops.read_with_offset().map_err(err)?;
        fv.op(offset, &op).map_err(err)?;
        s.max_height = s.max_height.max(fv.operand_stack_height());
        let next = ops.original_position();
        match op {
            Operator::Block { .. } | Operator::If { .. } | Operator::Try { .. } => {
                labels.push(Label { loop_body: None, caught: false })
            }
            Operator::Loop { .. } => labels.push(Label { loop_body: Some(next), caught: false }),
            Operator::TryTable { try_table } => {
                // Catch labels count from the block around the try_table.
                for catch in &try_table.catches {
                    let (Catch::One { label, .. }
                    | Catch::OneRef { label, .. }
                    | Catch::All { label }
                    | Catch::AllRef { label }) = *catch;
                    let i = labels.len() - 1 - label as usize;
                    match labels[i].loop_body {
                        _ if i == 0 => {}
                        Some(body) => s.marks.push((body, Mark::Restore)),
                        None => labels[i].caught = true,
                    }
                }
                labels.push(Label { loop_body: None, caught: false });
            }
            Operator::End | Operator::Delegate { .. } => {
                if labels.pop().is_some_and(|l| l.caught) {
                    s.marks.push((next, Mark::Restore));
                }
            }
            Operator::Catch { .. } | Operator::CatchAll => s.marks.push((next, Mark::Restore)),
            // A host function never calls back into wasm, so it leaves the height alone.
            Operator::Call { function_index } | Operator::ReturnCall { function_index }
                if function_index < imported_funcs => {}
            Operator::Call { .. } | Operator::CallIndirect { .. } | Operator::CallRef { .. } => {
                s.marks.push((next, Mark::Restore))
            }
            Operator::ReturnCall { .. } | Operator::ReturnCallIndirect { .. } | Operator::ReturnCallRef { .. } => {
                s.marks.push((offset, Mark::TailCall))
            }
            Operator::ContNew { .. }
            | Operator::ContBind { .. }
            | Operator::Suspend { .. }
            | Operator::Resume { .. }
            | Operator::ResumeThrow { .. }
            | Operator::ResumeThrowRef { .. }
            | Operator::Switch { .. } => return Err("stack switching is not supported".into()),
            _ => {}
        }
    }
    ops.finish().map_err(err)?;
    // Loop landings are found after the loop starts; one mark per offset is enough.
    s.marks.sort();
    s.marks.dedup_by_key(|m| m.0);
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime::{Engine, Instance, Module, Store, Trap};

    fn limited(wat: &str, limit: u32) -> Vec<u8> {
        let wasm = instrument(&wat::parse_str(wat).unwrap(), limit).unwrap();
        Validator::new_with_features(WasmFeatures::all()).validate_all(&wasm).unwrap();
        wasm
    }

    /// Call export `run` of `wat` limited to `limit` units: the height left afterwards, or the trap.
    fn run(wat: &str, limit: u32) -> Result<i32, Trap> {
        let engine = Engine::default();
        let module = Module::new(&engine, limited(wat, limit)).unwrap();
        let mut store = Store::new(&engine, ());
        let instance = Instance::new(&mut store, &module, &[]).unwrap();
        let run = instance.get_typed_func::<(), ()>(&mut store, "run").unwrap();
        let result = run.call(&mut store, ());
        let height = instance.get_global(&mut store, HEIGHT_EXPORT).unwrap().get(&mut store).i32().unwrap();
        result.map(|()| height).map_err(|e| *e.downcast_ref::<Trap>().unwrap())
    }

    /// The operators of defined function `index` of `wasm`, as `Debug` text.
    fn ops_of(wasm: &[u8], index: usize) -> Vec<String> {
        let mut bodies = Parser::new(0).parse_all(wasm).filter_map(|p| match p.unwrap() {
            Payload::CodeSectionEntry(body) => Some(body),
            _ => None,
        });
        let body = bodies.nth(index).unwrap();
        body.get_operators_reader().unwrap().into_iter().map(|op| format!("{:?}", op.unwrap())).collect()
    }

    fn height_export(wasm: &[u8]) -> u32 {
        for p in Parser::new(0).parse_all(wasm) {
            if let Payload::ExportSection(exports) = p.unwrap() {
                for e in exports {
                    let e = e.unwrap();
                    if e.name == HEIGHT_EXPORT {
                        return e.index;
                    }
                }
            }
        }
        panic!("no height export")
    }

    #[test]
    fn a_frame_costs_params_locals_operands_and_two() {
        // run: no params or locals, two operands for the call: 4. f: 2 params, 3 locals, 3 operands: 10.
        let wat = r#"(module
          (func $f (param i32 i64) (local f32 f32 f32)
            (local.get 0) (local.get 1) (local.get 0) (drop) (drop) (drop))
          (func (export "run") (call $f (i32.const 1) (i64.const 2))))"#;
        assert_eq!(run(wat, 14), Ok(4), "the caller's height is back after the call");
        assert_eq!(run(wat, 13), Err(Trap::UnreachableCodeReached));
        assert_eq!(run(wat, 3), Err(Trap::UnreachableCodeReached), "a frame above the limit traps at once");
    }

    #[test]
    fn every_kind_of_call_gives_the_height_back() {
        // Deepest path: run (5) -> call_indirect $pair_caller (4) -> $pair (4) = 13. The tail-call chains
        // are 1000 deep but replace their frame each time: run (5) + $down (5). A leak in any of them would
        // pass 13 within two iterations of the loop.
        let wat = r#"(module
          (type $v (func))
          (type $p (func (param i32)))
          (table 3 funcref)
          (elem (i32.const 0) $leaf $pair_caller $down_indirect)
          (func $leaf)
          (func $pair (result i32 i64) (i32.const 1) (i64.const 2))
          (func $pair_caller (call $pair) (drop) (drop))
          (func $down (param $n i32)
            (if (local.get $n) (then (return_call $down (i32.sub (local.get $n) (i32.const 1))))))
          (func $down_indirect (param $n i32)
            (if (local.get $n)
              (then (return_call_indirect (type $p) (i32.sub (local.get $n) (i32.const 1)) (i32.const 2)))))
          (func (export "run") (local $i i32)
            (loop $l
              (call $leaf)
              (call_indirect (type $v) (i32.const 1))
              (call $down (i32.const 1000))
              (call_indirect (type $p) (i32.const 1000) (i32.const 2))
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (br_if $l (i32.lt_u (local.get $i) (i32.const 1000))))))"#;
        assert_eq!(run(wat, 13), Ok(5));
        assert_eq!(run(wat, 12), Err(Trap::UnreachableCodeReached));
    }

    #[test]
    fn a_catch_handler_puts_the_height_back() {
        // Wasmtime runs exceptions only with its `gc` feature, so this checks where the bookkeeping goes:
        // after each call, after the block a catch lands behind, and at the start of a loop a catch lands in.
        let wat = r#"(module
          (tag $e)
          (func $throw (throw $e))
          (func (export "run") (local $i i32)
            (block $caught
              (try_table (catch $e $caught) (call $throw)))
            (loop $again
              (block $out
                (try_table (catch_all $again)
                  (br_if $out (local.get $i))
                  (local.set $i (i32.const 1))
                  (call $throw))))))"#;
        let ops = ops_of(&limited(wat, 100), 1);
        let restore = ["LocalGet { local_index: 1 }", "GlobalSet { global_index: 0 }"];
        let before: Vec<&str> = (1..ops.len() - 1)
            .filter(|&i| ops[i] == restore[0] && ops[i + 1] == restore[1])
            .map(|i| ops[i - 1].split(' ').next().unwrap())
            .collect();
        assert_eq!(before, ["Call", "End", "Loop", "Call"], "{ops:#?}");
    }

    #[test]
    fn the_height_global_and_its_export_are_added_where_the_module_has_none() {
        let wasm = limited(r#"(module (func $f) (start $f))"#, 10);
        assert_eq!(height_export(&wasm), 0);
        let wasm =
            limited(r#"(module (import "m" "g" (global i32)) (global i32 (i32.const 5)) (func (export "f")))"#, 10);
        assert_eq!(height_export(&wasm), 2, "after the imported and the defined global");
        let wasm = wat::parse_str(r#"(module (global (export "__ibc_stack_height") i32 (i32.const 0)))"#).unwrap();
        assert!(instrument(&wasm, 10).unwrap_err().contains("reserved"));
    }
}
