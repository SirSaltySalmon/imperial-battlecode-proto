//! Upload-time validation of a bot module, without wasmtime: the same rules as `ibc_runner::check_wasm`
//! (imports only from `ibc` and the supported WASI subset, exports `_start: () -> ()` and `memory`) plus a
//! size cap and a cap on the declared initial memory. A module that passes here can still be rejected by the
//! runner's compiler (the worker then fails the job with `retryable: false`); this check exists to turn the
//! common mistakes into an immediate, readable upload error.
//!
//! What a bot may import depends on the patch (`Abi`): the coordinator re-runs this check on every live
//! submission when the current patch changes (`crate::patch`). `detect_languages` reads what a module says
//! about the language it was built from.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use wasmparser::{ExternalKind, KnownCustom, Parser, Payload, TypeRef, Validator, WasmFeatures};

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

/// The imports a patch offers a bot. The defaults are this build's lists (`IBC_IMPORTS`, `WASI_IMPORTS`); a
/// patch registered with an `imports` object overrides either list (docs/release.md, "Revalidation").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Abi {
    /// Functions of module `ibc`.
    #[serde(default = "default_ibc")]
    pub ibc: Vec<String>,
    /// Functions of module `wasi_snapshot_preview1`.
    #[serde(default = "default_wasi")]
    pub wasi: Vec<String>,
}

fn default_ibc() -> Vec<String> {
    IBC_IMPORTS.iter().map(|s| s.to_string()).collect()
}

fn default_wasi() -> Vec<String> {
    WASI_IMPORTS.iter().map(|s| s.to_string()).collect()
}

impl Default for Abi {
    fn default() -> Abi {
        Abi {
            ibc: default_ibc(),
            wasi: default_wasi(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_bytes: usize,
    /// Cap on the initial linear memory: the ruleset's `memory_bytes` (the runner also caps growth with it).
    pub max_memory_bytes: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_bytes: 16 << 20,
            max_memory_bytes: 8 << 20,
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

/// `check_against` this build's ABI.
pub fn check_wasm(wasm: &[u8], limits: &Limits) -> Result<(), String> {
    check_against(wasm, limits, &Abi::default())
}

/// `Ok(())` or a message fit to show to the team that uploaded the module.
pub fn check_against(wasm: &[u8], limits: &Limits, abi: &Abi) -> Result<(), String> {
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
                        IBC_MODULE => abi.ibc.iter().any(|n| n == import.name),
                        WASI_MODULE => abi.wasi.iter().any(|n| n == import.name),
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

/// Custom section in which a toolchain names the language of a module, as UTF-8 text (`rust`, `cpp`,
/// `python`). When present it takes precedence over the `producers` section.
pub const LANGUAGE_SECTION: &str = "ibc.language";

/// The coordinator's name for a language or compiler named in a `producers` section, if it knows it.
fn producer_language(field: &str, name: &str) -> Option<&'static str> {
    let name = name.to_ascii_lowercase();
    // C and C++ share the C++ toolchain (`ibc build` of a C++ bot): C99, C11, C17, C++14, ...
    let c_family = name == "c"
        || name.starts_with("c++")
        || name.starts_with("c_plus_plus")
        || (name.len() > 1
            && name.starts_with('c')
            && name[1..].bytes().all(|b| b.is_ascii_digit()));
    match field {
        "language" if name == "rust" => Some("rust"),
        "language" if c_family => Some("cpp"),
        "processed-by" if name == "rustc" => Some("rust"),
        "processed-by" if name == "clang" => Some("cpp"),
        _ => None,
    }
}

/// The languages a module says it was built from: the `ibc.language` custom section if it has one,
/// otherwise the languages and compilers of its `producers` section (WebAssembly tool conventions), in the
/// coordinator's names. Empty when the module says nothing (release builds often strip both). This catches
/// mistakes; it proves nothing, since a module can be stripped or carry any section.
pub fn detect_languages(wasm: &[u8]) -> BTreeSet<String> {
    let mut declared = BTreeSet::new();
    let mut produced = BTreeSet::new();
    for payload in Parser::new(0).parse_all(wasm) {
        let Ok(Payload::CustomSection(section)) = payload else {
            continue;
        };
        if section.name() == LANGUAGE_SECTION {
            if let Ok(text) = std::str::from_utf8(section.data()) {
                declared.insert(text.trim().to_ascii_lowercase());
            }
            continue;
        }
        let KnownCustom::Producers(reader) = section.as_known() else {
            continue;
        };
        for field in reader.into_iter().flatten() {
            for value in field.values.into_iter().flatten() {
                if let Some(lang) = producer_language(field.name, value.name) {
                    produced.insert(lang.to_string());
                }
            }
        }
    }
    if declared.is_empty() {
        produced
    } else {
        declared
    }
}
