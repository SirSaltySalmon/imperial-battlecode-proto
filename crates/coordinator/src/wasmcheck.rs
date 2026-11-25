//! Upload-time validation of a bot module, without wasmtime: the same rules as `ibc_runner::check_wasm`
//! (imports only from `ibc` and the supported WASI subset, exports `_start: () -> ()` and `memory`) plus a
//! size cap and a cap on the declared initial memory. A module that passes here can still be rejected by the
//! runner's compiler (the worker then fails the job with `retryable: false`); this check exists to turn the
//! common mistakes into an immediate, readable upload error.

use std::collections::HashMap;
use wasmparser::{ExternalKind, Parser, Payload, TypeRef, Validator, WasmFeatures};

/// Module name of the game imports (`ibc_abi::IMPORT_MODULE`).
pub const IBC_MODULE: &str = "ibc";
/// `ibc_runner::IBC_IMPORTS` (listed at the top of `crates/abi/src/lib.rs`).
pub const IBC_IMPORTS: &[&str] = &[
    "end_turn",
    "fuel_remaining",
    "count",
    "get",
    "act",
    "los",
    "raycast",
    "mem_read",
    "mem_write",
    "debug_draw",
    "log",
];
pub const WASI_MODULE: &str = "wasi_snapshot_preview1";
/// `ibc_runner::WASI_IMPORTS` (`crates/runner/src/wasi.rs`).
pub const WASI_IMPORTS: &[&str] = &[
    "fd_write",
    "proc_exit",
    "environ_sizes_get",
    "environ_get",
    "args_sizes_get",
    "args_get",
    "clock_time_get",
    "random_get",
    "fd_close",
    "fd_seek",
    "fd_fdstat_get",
    "fd_prestat_get",
    "fd_prestat_dir_name",
    "fd_read",
    "path_open",
    "sched_yield",
    "poll_oneoff",
];

const PAGE: u64 = 65_536;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_bytes: usize,
    /// Cap on the initial linear memory (the runner caps growth at run time: `MEMORY_LIMIT_BYTES`).
    pub max_memory_bytes: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_bytes: 16 << 20,
            max_memory_bytes: 64 << 20,
        }
    }
}

/// The parser's message on one line.
fn invalid(e: wasmparser::BinaryReaderError) -> String {
    format!(
        "invalid wasm: {}",
        e.message().split_whitespace().collect::<Vec<_>>().join(" ")
    )
}

/// `Ok(())` or a message fit to show to the team that uploaded the module.
pub fn check_wasm(wasm: &[u8], limits: &Limits) -> Result<(), String> {
    if wasm.is_empty() {
        return Err("empty upload".into());
    }
    if wasm.len() > limits.max_bytes {
        return Err(format!(
            "module is {} bytes, the limit is {}",
            wasm.len(),
            limits.max_bytes
        ));
    }
    // WebAssembly 2.0: what a `wasm32-wasip1` toolchain emits. No threads, GC, memory64 or components.
    Validator::new_with_features(WasmFeatures::WASM2)
        .validate_all(wasm)
        .map_err(invalid)?;

    let bad = invalid;
    let mut types: Vec<(usize, usize)> = Vec::new(); // (params, results) per type index
    let mut funcs: Vec<u32> = Vec::new(); // type index per function index (imports first)
    let mut memories: Vec<wasmparser::MemoryType> = Vec::new();
    let mut exports: HashMap<String, (ExternalKind, u32)> = HashMap::new();
    let mut unsupported: Vec<String> = Vec::new();

    for payload in Parser::new(0).parse_all(wasm) {
        match payload.map_err(bad)? {
            Payload::TypeSection(reader) => {
                for ty in reader.into_iter_err_on_gc_types() {
                    let ty = ty.map_err(bad)?;
                    types.push((ty.params().len(), ty.results().len()));
                }
            }
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.map_err(bad)?;
                    let known = match import.module {
                        IBC_MODULE => IBC_IMPORTS.contains(&import.name),
                        WASI_MODULE => WASI_IMPORTS.contains(&import.name),
                        _ => false,
                    };
                    match import.ty {
                        TypeRef::Func(ty) if known => funcs.push(ty),
                        TypeRef::Func(ty) => {
                            funcs.push(ty);
                            unsupported.push(format!("{}.{}", import.module, import.name));
                        }
                        _ => unsupported.push(format!("{}.{}", import.module, import.name)),
                    }
                }
            }
            Payload::FunctionSection(reader) => {
                for ty in reader {
                    funcs.push(ty.map_err(bad)?);
                }
            }
            Payload::MemorySection(reader) => {
                for m in reader {
                    memories.push(m.map_err(bad)?);
                }
            }
            Payload::ExportSection(reader) => {
                for e in reader {
                    let e = e.map_err(bad)?;
                    exports.insert(e.name.to_string(), (e.kind, e.index));
                }
            }
            _ => {}
        }
    }

    if !unsupported.is_empty() {
        return Err(format!("unsupported imports: {}", unsupported.join(", ")));
    }
    let start_ok = match exports.get("_start") {
        Some(&(ExternalKind::Func, index)) => funcs
            .get(index as usize)
            .and_then(|&ty| types.get(ty as usize))
            .is_some_and(|&(params, results)| params == 0 && results == 0),
        _ => false,
    };
    if !start_ok {
        return Err(
            "missing export `_start` of type () -> () (build a wasm32-wasip1 command module)"
                .into(),
        );
    }
    // Imported memories were rejected above, so the memory index space is the memory section.
    let memory = match exports.get("memory") {
        Some(&(ExternalKind::Memory, index)) => memories.get(index as usize),
        _ => None,
    };
    let Some(memory) = memory else {
        return Err("missing export `memory`".into());
    };
    let initial = memory.initial.saturating_mul(PAGE);
    if initial > limits.max_memory_bytes {
        return Err(format!(
            "initial memory is {initial} bytes, the limit is {} (the runner caps a bot at that size)",
            limits.max_memory_bytes
        ));
    }
    Ok(())
}
