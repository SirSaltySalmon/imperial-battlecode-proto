//! Native backend: run bot code directly against an `ibc_abi::Host` (the engine), no wasm involved.
//!
//! ```ignore
//! for unit in m.turn_order() { ibc::native::with_host(&mut m, unit, || bots[unit as usize].turn()); }
//! ```
use ibc_abi::Host;
use std::cell::{Cell, RefCell};

/// Counters of `act` calls made through the native backend on this thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub act_calls: u64,
    pub act_errors: u64,
    /// Errors indexed by action kind (`A_*`); the last slot collects unknown kinds.
    pub errors_by_kind: [u64; 16],
}

thread_local! {
    static CURRENT: Cell<Option<(*mut dyn Host, u32)>> = const { Cell::new(None) };
    pub(crate) static STATS: RefCell<Stats> = RefCell::new(Stats::default());
    static LOG: Cell<bool> = const { Cell::new(false) };
}

struct Restore(Option<(*mut dyn Host, u32)>);
impl Drop for Restore {
    fn drop(&mut self) {
        CURRENT.with(|c| c.set(self.0));
    }
}

/// Run `f` with every SDK call on this thread dispatched to `host` as unit `unit`.
pub fn with_host<H: Host + 'static, R>(host: &mut H, unit: u32, f: impl FnOnce() -> R) -> R {
    let ptr: *mut dyn Host = host;
    let _restore = Restore(CURRENT.with(|c| c.replace(Some((ptr, unit)))));
    f()
}

pub(crate) fn with_current<R>(f: impl FnOnce(&mut dyn Host, u32) -> R) -> Option<R> {
    // Take the host out for the duration of the call so a re-entrant call cannot alias the &mut.
    let (ptr, unit) = CURRENT.with(|c| c.take())?;
    let _restore = Restore(Some((ptr, unit)));
    // SAFETY: `ptr` comes from the `&mut H` borrowed for the whole `with_host` call on this thread,
    // and it is removed from the slot while this unique reference is live.
    Some(f(unsafe { &mut *ptr }, unit))
}

/// Unit id the current thread is acting as, if inside `with_host`.
pub fn current_unit() -> Option<u32> {
    CURRENT.with(|c| c.get().map(|(_, u)| u))
}

pub fn stats() -> Stats {
    STATS.with(|s| *s.borrow())
}

pub fn reset_stats() {
    STATS.with(|s| *s.borrow_mut() = Stats::default());
}

/// Print `ibc::log` output to stderr (off by default).
pub fn set_log(enabled: bool) {
    LOG.with(|l| l.set(enabled));
}

pub(crate) fn emit_log(msg: &[u8]) {
    if LOG.with(|l| l.get()) {
        let unit = current_unit().map(|u| u as i64).unwrap_or(-1);
        eprintln!("[unit {unit}] {}", String::from_utf8_lossy(msg));
    }
}
