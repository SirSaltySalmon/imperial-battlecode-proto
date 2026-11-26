//! The engine for the browser. Build with
//! `cargo build -p ibc-engine-wasm --release --target wasm32-unknown-unknown`; the result has no imports.
//!
//! JavaScript usage (all pointers are offsets into the exported `memory`):
//!   p = ibc_alloc(n); copy the .ibcr bytes to p; rc = ibc_load(p, n); ibc_free(p, n)
//!   rc == 0: ibc_frame_count() frames (initial state + one per tick); frame i is UTF-8 JSON at
//!            ibc_frame_ptr(i) / ibc_frame_len(i) (same object as `ibc replay dump` prints per line);
//!            the replay header is JSON at ibc_header_ptr() / ibc_header_len().
//!   rc != 0: the message is at ibc_error_ptr() / ibc_error_len(). Loading re-simulates the whole match and
//!            checks every state hash, so a replay from another engine version fails here instead of drifting.
//!            The private records of the teams the replay still has (logs, per-turn fuel, debug drawings; see
//!            `ibc_replay::records_json`) are JSON at ibc_records_ptr() / ibc_records_len().
//!   The rules (sizes, timings, weapon names by id) are JSON at ibc_rules_ptr() / ibc_rules_len(): those of
//!   the loaded replay (its header rules, or the built-in rules for a replay without them), or the built-in
//!   rules when no replay is loaded.
//! Each engine release ships its own build of this module, so old replays stay viewable.

use std::cell::RefCell;

#[derive(Default)]
struct Loaded {
    frames: Vec<String>,
    header: String,
    records: String,
    /// Rules JSON of the loaded replay; empty means the built-in rules.
    rules: String,
    error: String,
}

thread_local! {
    static STATE: RefCell<Loaded> = RefCell::new(Loaded::default());
    static RULES: String = rules();
}

/// The built-in ruleset as the JSON a viewer reads (`ibc_engine::rules_json`).
pub fn rules() -> String {
    ibc_engine::rules_json(&ibc_engine::Ruleset::builtin())
}

pub struct Replay {
    pub frames: Vec<String>,
    pub header: String,
    /// `ibc_replay::records_json` without memory writes.
    pub records: String,
    /// `ibc_engine::rules_json` of the rules the replay was simulated with.
    pub rules: String,
}

/// Decode and re-simulate a replay.
pub fn load(bytes: &[u8]) -> Result<Replay, String> {
    let replay = ibc_replay::Replay::decode(bytes)?;
    let mut frames = Vec::with_capacity(replay.ticks.len() + 1);
    let m = replay.simulate(|m| frames.push(m.frame_json()))?;
    let header = serde_json::to_string(&replay.header).map_err(|e| e.to_string())?;
    let records = ibc_replay::records_json(&replay, false);
    Ok(Replay { frames, header, records, rules: ibc_engine::rules_json(&m.world.rules) })
}

#[no_mangle]
pub extern "C" fn ibc_alloc(len: u32) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len.max(1) as usize);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// # Safety
/// `ptr` and `len` must come from one `ibc_alloc(len)` call that has not been freed.
#[no_mangle]
pub unsafe extern "C" fn ibc_free(ptr: *mut u8, len: u32) {
    drop(Vec::from_raw_parts(ptr, 0, len.max(1) as usize));
}

/// # Safety
/// `ptr..ptr+len` must be readable.
#[no_mangle]
pub unsafe extern "C" fn ibc_load(ptr: *const u8, len: u32) -> i32 {
    let bytes = std::slice::from_raw_parts(ptr, len as usize);
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        *s = Loaded::default();
        match load(bytes) {
            Ok(r) => {
                s.frames = r.frames;
                s.header = r.header;
                s.records = r.records;
                s.rules = r.rules;
                0
            }
            Err(e) => {
                s.error = e;
                1
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn ibc_frame_count() -> u32 {
    STATE.with(|s| s.borrow().frames.len() as u32)
}

#[no_mangle]
pub extern "C" fn ibc_frame_ptr(i: u32) -> *const u8 {
    STATE.with(|s| s.borrow().frames.get(i as usize).map_or(std::ptr::null(), |f| f.as_ptr()))
}

#[no_mangle]
pub extern "C" fn ibc_frame_len(i: u32) -> u32 {
    STATE.with(|s| s.borrow().frames.get(i as usize).map_or(0, |f| f.len() as u32))
}

#[no_mangle]
pub extern "C" fn ibc_header_ptr() -> *const u8 {
    STATE.with(|s| s.borrow().header.as_ptr())
}

#[no_mangle]
pub extern "C" fn ibc_header_len() -> u32 {
    STATE.with(|s| s.borrow().header.len() as u32)
}

#[no_mangle]
pub extern "C" fn ibc_error_ptr() -> *const u8 {
    STATE.with(|s| s.borrow().error.as_ptr())
}

#[no_mangle]
pub extern "C" fn ibc_error_len() -> u32 {
    STATE.with(|s| s.borrow().error.len() as u32)
}

#[no_mangle]
pub extern "C" fn ibc_records_ptr() -> *const u8 {
    STATE.with(|s| s.borrow().records.as_ptr())
}

#[no_mangle]
pub extern "C" fn ibc_records_len() -> u32 {
    STATE.with(|s| s.borrow().records.len() as u32)
}

#[no_mangle]
pub extern "C" fn ibc_rules_ptr() -> *const u8 {
    STATE.with(|s| {
        let s = s.borrow();
        if s.rules.is_empty() {
            RULES.with(|r| r.as_ptr())
        } else {
            s.rules.as_ptr()
        }
    })
}

#[no_mangle]
pub extern "C" fn ibc_rules_len() -> u32 {
    STATE.with(|s| {
        let s = s.borrow();
        if s.rules.is_empty() {
            RULES.with(|r| r.len() as u32)
        } else {
            s.rules.len() as u32
        }
    })
}

/// Engine version of this build, as a NUL-free UTF-8 string of `ibc_version_len()` bytes.
#[no_mangle]
pub extern "C" fn ibc_version_ptr() -> *const u8 {
    ibc_engine::ENGINE_VERSION.as_ptr()
}

#[no_mangle]
pub extern "C" fn ibc_version_len() -> u32 {
    ibc_engine::ENGINE_VERSION.len() as u32
}
