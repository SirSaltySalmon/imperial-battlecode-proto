//! Raw host calls. Two backends with identical signatures:
//! wasm imports from module `"ibc"`, or a thread-local `ibc_abi::Host` when built natively.

#[cfg(target_arch = "wasm32")]
mod imp {
    #[link(wasm_import_module = "ibc")]
    extern "C" {
        fn end_turn();
        fn fuel_remaining() -> i64;
        fn count(table: u32) -> i32;
        fn get(table: u32, index: u32, out: *mut u8, cap: u32) -> i32;
        fn act(kind: u32, a: i32, b: i32, c: i32) -> i32;
        fn los(x0: i32, y0: i32, x1: i32, y1: i32) -> i32;
        fn raycast(x: i32, y: i32, angle: i32, max: i32) -> i32;
        fn mem_read(scope: u32, off: u32, ptr: *mut u8, len: u32) -> i32;
        fn mem_write(scope: u32, off: u32, ptr: *const u8, len: u32) -> i32;
        fn debug_draw(kind: u32, x0: i32, y0: i32, x1: i32, y1: i32, color: u32) -> i32;
        fn log(ptr: *const u8, len: u32) -> i32;
    }

    // SAFETY (all): plain imports; pointers come from live slices with matching lengths.
    pub fn sys_end_turn() {
        unsafe { end_turn() }
    }
    pub fn sys_fuel_remaining() -> i64 {
        unsafe { fuel_remaining() }
    }
    pub fn sys_count(table: u32) -> i32 {
        unsafe { count(table) }
    }
    pub fn sys_get(table: u32, index: u32, out: &mut [u8]) -> i32 {
        unsafe { get(table, index, out.as_mut_ptr(), out.len() as u32) }
    }
    pub fn sys_act(kind: u32, a: i32, b: i32, c: i32) -> i32 {
        unsafe { act(kind, a, b, c) }
    }
    pub fn sys_los(x0: i32, y0: i32, x1: i32, y1: i32) -> i32 {
        unsafe { los(x0, y0, x1, y1) }
    }
    pub fn sys_raycast(x: i32, y: i32, angle: i32, max: i32) -> i32 {
        unsafe { raycast(x, y, angle, max) }
    }
    pub fn sys_mem_read(scope: u32, off: u32, out: &mut [u8]) -> i32 {
        unsafe { mem_read(scope, off, out.as_mut_ptr(), out.len() as u32) }
    }
    pub fn sys_mem_write(scope: u32, off: u32, data: &[u8]) -> i32 {
        unsafe { mem_write(scope, off, data.as_ptr(), data.len() as u32) }
    }
    pub fn sys_debug_draw(kind: u32, x0: i32, y0: i32, x1: i32, y1: i32, color: u32) -> i32 {
        unsafe { debug_draw(kind, x0, y0, x1, y1, color) }
    }
    pub fn sys_log(msg: &[u8]) -> i32 {
        unsafe { log(msg.as_ptr(), msg.len() as u32) }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use crate::native::{with_current, STATS};
    use ibc_abi::ERR_NOT_ALLOWED;

    pub fn sys_end_turn() {}
    pub fn sys_fuel_remaining() -> i64 {
        i64::MAX
    }
    pub fn sys_count(table: u32) -> i32 {
        with_current(|h, u| h.count(u, table)).unwrap_or(-ERR_NOT_ALLOWED)
    }
    pub fn sys_get(table: u32, index: u32, out: &mut [u8]) -> i32 {
        with_current(|h, u| {
            let mut buf = Vec::with_capacity(out.len());
            let n = h.get(u, table, index, &mut buf);
            if n < 0 {
                return n;
            }
            // Records grow at the end: a caller built against an older ABI gets the prefix it knows.
            buf.truncate(out.len());
            out[..buf.len()].copy_from_slice(&buf);
            buf.len() as i32
        })
        .unwrap_or(-ERR_NOT_ALLOWED)
    }
    pub fn sys_act(kind: u32, a: i32, b: i32, c: i32) -> i32 {
        let r = with_current(|h, u| h.act(u, kind, a, b, c)).unwrap_or(ERR_NOT_ALLOWED);
        STATS.with(|s| {
            let mut s = s.borrow_mut();
            s.act_calls += 1;
            if r != 0 {
                s.act_errors += 1;
                let k = (kind as usize).min(s.errors_by_kind.len() - 1);
                s.errors_by_kind[k] += 1;
            }
        });
        r
    }
    pub fn sys_los(x0: i32, y0: i32, x1: i32, y1: i32) -> i32 {
        with_current(|h, _| h.los(x0, y0, x1, y1)).unwrap_or(0)
    }
    pub fn sys_raycast(x: i32, y: i32, angle: i32, max: i32) -> i32 {
        with_current(|h, _| h.raycast(x, y, angle, max)).unwrap_or(0)
    }
    pub fn sys_mem_read(scope: u32, off: u32, out: &mut [u8]) -> i32 {
        with_current(|h, u| h.mem_read(u, scope, off, out)).unwrap_or(ERR_NOT_ALLOWED)
    }
    pub fn sys_mem_write(scope: u32, off: u32, data: &[u8]) -> i32 {
        with_current(|h, u| h.mem_write(u, scope, off, data)).unwrap_or(ERR_NOT_ALLOWED)
    }
    pub fn sys_debug_draw(kind: u32, x0: i32, y0: i32, x1: i32, y1: i32, color: u32) -> i32 {
        with_current(|h, u| h.debug_draw(u, kind, x0, y0, x1, y1, color)).unwrap_or(ERR_NOT_ALLOWED)
    }
    pub fn sys_log(msg: &[u8]) -> i32 {
        crate::native::emit_log(msg);
        0
    }
}

pub use imp::*;
