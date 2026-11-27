//! A hand-written, deterministic WASI preview1 shim: just enough for `wasm32-wasip1` command modules
//! (Rust std, wasi-libc) to start, print and abort. No filesystem, no clock, no entropy from the host.

use crate::vm::{charge_now, log_guest, read_bytes, write_bytes, Exit, UnitData};
use wasmtime::{Caller, Linker};

pub const MODULE: &str = "wasi_snapshot_preview1";

const ESUCCESS: i32 = 0;
const EBADF: i32 = 8;
const EFAULT: i32 = 21;
const EINVAL: i32 = 28;
const ENOSYS: i32 = 52;

/// Most iovecs honoured per `fd_write`, and most bytes per `random_get`: both bound the host work of a call,
/// and so its fee.
const MAX_IOVS: u32 = 64;
const MAX_RANDOM: u32 = 4096;

/// Every import of `wasi_snapshot_preview1` this host provides.
pub const SUPPORTED: &[&str] = &[
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

fn put(caller: &mut Caller<'_, UnitData>, ptr: u32, bytes: &[u8]) -> i32 {
    if write_bytes(caller, ptr, bytes) {
        ESUCCESS
    } else {
        EFAULT
    }
}

fn sizes_get(mut caller: Caller<'_, UnitData>, count: u32, size: u32) -> i32 {
    let a = put(&mut caller, count, &0u32.to_le_bytes());
    let b = put(&mut caller, size, &0u32.to_le_bytes());
    a.max(b)
}

fn fd_write(
    mut caller: Caller<'_, UnitData>,
    fd: i32,
    iovs: u32,
    n: u32,
    out: u32,
) -> wasmtime::Result<i32> {
    charge_now(&mut caller, ibc_abi::FUEL_IOV * u64::from(n.min(MAX_IOVS)))?;
    if fd != 1 && fd != 2 {
        return Ok(EBADF);
    }
    let mut written = 0u32;
    for i in 0..n.min(MAX_IOVS) {
        let mut iov = [0u8; 8];
        let Some(at) = iovs.checked_add(i * 8) else {
            return Ok(EFAULT);
        };
        if !read_bytes(&caller, at, &mut iov) {
            return Ok(EFAULT);
        }
        let ptr = u32::from_le_bytes(iov[..4].try_into().unwrap());
        let len = u32::from_le_bytes(iov[4..].try_into().unwrap());
        if !log_guest(&mut caller, ptr, len, false) {
            return Ok(EFAULT);
        }
        written = written.saturating_add(len);
    }
    Ok(put(&mut caller, out, &written.to_le_bytes()))
}

fn random_get(mut caller: Caller<'_, UnitData>, buf: u32, len: u32) -> wasmtime::Result<i32> {
    charge_now(
        &mut caller,
        ibc_abi::FUEL_RANDOM * u64::from(len.min(MAX_RANDOM).div_ceil(8)),
    )?;
    if len > MAX_RANDOM {
        return Ok(EINVAL);
    }
    let mut bytes = vec![0u8; len as usize];
    caller.data_mut().fill_random(&mut bytes);
    Ok(put(&mut caller, buf, &bytes))
}

pub fn add_to_linker(l: &mut Linker<UnitData>) -> wasmtime::Result<()> {
    l.func_wrap(MODULE, "fd_write", fd_write)?;
    l.func_wrap(MODULE, "proc_exit", |mut caller: Caller<'_, UnitData>, code: i32| -> wasmtime::Result<()> {
        caller.data_mut().exit = Some(Exit::ProcExit(code));
        Err(wasmtime::Error::msg("proc_exit"))
    })?;
    l.func_wrap(MODULE, "environ_sizes_get", sizes_get)?;
    l.func_wrap(MODULE, "args_sizes_get", sizes_get)?;
    l.func_wrap(MODULE, "environ_get", |_: Caller<'_, UnitData>, _: u32, _: u32| -> i32 { ESUCCESS })?;
    l.func_wrap(MODULE, "args_get", |_: Caller<'_, UnitData>, _: u32, _: u32| -> i32 { ESUCCESS })?;
    l.func_wrap(MODULE, "clock_time_get", |mut caller: Caller<'_, UnitData>, _id: i32, _prec: i64, out: u32| -> i32 {
        // Every clock reads the tick number: 1 tick = 1 ms.
        let now = caller.data().tick() as u64 * 1_000_000;
        put(&mut caller, out, &now.to_le_bytes())
    })?;
    l.func_wrap(MODULE, "random_get", random_get)?;
    l.func_wrap(MODULE, "fd_close", |_: Caller<'_, UnitData>, _fd: i32| -> i32 { EBADF })?;
    l.func_wrap(MODULE, "fd_seek", |_: Caller<'_, UnitData>, _fd: i32, _off: i64, _whence: i32, _out: u32| -> i32 {
        EBADF
    })?;
    l.func_wrap(MODULE, "fd_fdstat_get", |mut caller: Caller<'_, UnitData>, fd: i32, out: u32| -> i32 {
        if !(0..=2).contains(&fd) {
            return EBADF;
        }
        // fdstat: filetype u8 (2 = character device), flags u16 @2, rights_base u64 @8, rights_inheriting u64 @16
        let mut stat = [0u8; 24];
        stat[0] = 2;
        let rights: u64 = if fd == 0 { 1 << 1 } else { 1 << 6 }; // fd_read / fd_write
        stat[8..16].copy_from_slice(&rights.to_le_bytes());
        put(&mut caller, out, &stat)
    })?;
    l.func_wrap(MODULE, "fd_prestat_get", |_: Caller<'_, UnitData>, _fd: i32, _out: u32| -> i32 { EBADF })?;
    l.func_wrap(MODULE, "fd_prestat_dir_name", |_: Caller<'_, UnitData>, _fd: i32, _p: u32, _l: u32| -> i32 { EBADF })?;
    l.func_wrap(MODULE, "fd_read", |mut caller: Caller<'_, UnitData>, fd: i32, _iovs: u32, _n: u32, out: u32| -> i32 {
        if fd != 0 {
            return EBADF;
        }
        put(&mut caller, out, &0u32.to_le_bytes()) // stdin is always at end of file
    })?;
    l.func_wrap(
        MODULE,
        "path_open",
        |_: Caller<'_, UnitData>, _fd: i32, _df: i32, _p: u32, _pl: u32, _of: i32, _rb: i64, _ri: i64, _ff: i32, _out: u32| -> i32 {
            EBADF
        },
    )?;
    l.func_wrap(MODULE, "sched_yield", |_: Caller<'_, UnitData>| -> i32 { ESUCCESS })?;
    l.func_wrap(MODULE, "poll_oneoff", |_: Caller<'_, UnitData>, _in: u32, _out: u32, _n: u32, _ev: u32| -> i32 {
        ENOSYS
    })?;
    Ok(())
}
