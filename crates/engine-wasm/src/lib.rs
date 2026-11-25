//! The engine for the browser. Build with
//! `cargo build -p ibc-engine-wasm --release --target wasm32-unknown-unknown`; the result has no imports.
//!
//! JavaScript usage (all pointers are offsets into the exported `memory`):
//!   p = ibc_alloc(n); copy the .ibcr bytes to p; rc = ibc_load(p, n); ibc_free(p, n)
//!   rc == 0: ibc_frame_count() frames (initial state + one per tick); frame i is UTF-8 JSON at
//!            ibc_frame_ptr(i) / ibc_frame_len(i) (the object `ibc replay dump` prints per line, plus `loadout`);
//!            the replay header is JSON at ibc_header_ptr() / ibc_header_len().
//!   rc != 0: the message is at ibc_error_ptr() / ibc_error_len(). Loading re-simulates the whole match and
//!            checks every state hash, so a replay from another engine version fails here instead of drifting.
//!   Each frame also carries `loadout` (both weapon slots, ammo, ability charges per unit; see `view.rs`).
//!   The rules this build simulates with (sizes, timings, weapon names by id, and every weapon, ability, kit,
//!   shop item and economy number) are JSON at ibc_rules_ptr() / ibc_rules_len(), available without loading
//!   a replay.
//! Each engine release ships its own build of this module, so old replays stay viewable.

use std::cell::RefCell;

mod view;

#[derive(Default)]
struct Loaded {
    frames: Vec<String>,
    header: String,
    error: String,
}

thread_local! {
    static STATE: RefCell<Loaded> = RefCell::new(Loaded::default());
    static RULES: String = rules();
}

/// The built-in ruleset as the JSON a viewer reads (`ibc_engine::rules_json` plus the full tables).
pub fn rules() -> String {
    view::rules(&ibc_engine::Ruleset::builtin())
}

/// Decode and re-simulate a replay. Returns the frames and the header JSON.
pub fn load(bytes: &[u8]) -> Result<(Vec<String>, String), String> {
    let replay = ibc_replay::Replay::decode(bytes)?;
    let mut frames = Vec::with_capacity(replay.ticks.len() + 1);
    replay.simulate(|m| frames.push(view::frame(&m.world, m.frame_json())))?;
    let header = serde_json::to_string(&replay.header).map_err(|e| e.to_string())?;
    Ok((frames, header))
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
            Ok((frames, header)) => {
                s.frames = frames;
                s.header = header;
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
pub extern "C" fn ibc_rules_ptr() -> *const u8 {
    RULES.with(|r| r.as_ptr())
}

#[no_mangle]
pub extern "C" fn ibc_rules_len() -> u32 {
    RULES.with(|r| r.len() as u32)
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
