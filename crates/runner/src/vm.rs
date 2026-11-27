//! One wasm instance per unit, run as a suspended `_start` call with an exact per-turn fuel budget.
//!
//! # Fuel model
//!
//! Wasmtime counts fuel down from a huge initial amount (`INITIAL_FUEL`); a store's `get_fuel()` is therefore
//! an exact odometer of everything the unit ever consumed. A turn with budget `B` is the interval
//! `[floor, start]` of that odometer where `floor = start - B`:
//!
//! * the driver arms an async yield slice of exactly `B` before resuming the bot, so wasm that reaches the
//!   floor suspends inside Wasmtime's out-of-fuel hook and resumes at the same instruction next turn;
//! * a host call with cost `c` moves the odometer down by `c` and re-arms the slice to what is left, so host
//!   charges and wasm instructions draw on the same budget. If no more than `c` is left the call itself
//!   suspends until the next turn (the rest of this turn's fuel is discarded). If `c` is at least the
//!   turn's whole budget, the call takes what is left and owes the rest as debt (below). WASI calls are paid
//!   like instructions instead: at once, and what runs past the floor is debt;
//! * `end_turn` suspends; unused fuel is discarded because the next turn computes a fresh floor.
//!
//! Wasmtime only checks fuel at loop headers and function entries, so a unit can run past the floor by the
//! cost of one check-free path: its instructions, plus the bytes of any `memory.copy`/`memory.fill` and the
//! WASI fees on it, or an `ibc` fee as large as a turn. The driver reads the odometer after every turn and
//! subtracts that overshoot (the "debt") from the next budget, so total consumption never exceeds total
//! budget by more than one such path, no matter how long the match is.

use ibc_abi::Host as _;
use ibc_engine::Match;
use std::future::Future;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use wasmtime::{
    Caller, Engine, Global, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, ThrownException, Trap, Val,
};

/// Fuel put into every store once, after instantiation. Never runs out in practice (9e18).
pub(crate) const INITIAL_FUEL: u64 = u64::MAX / 2;
/// Fuel available to instantiation (data segment initialisation, wasm start function).
const INSTANTIATE_FUEL: u64 = 1_000_000;
/// Largest `mem_read` / `mem_write` length passed on to the engine.
const MAX_MEM_IO: u32 = 1 << 20;
/// Most elements of a unit's one table (the function table of indirect calls; toolchains emit a few dozen).
const MAX_TABLE_ELEMENTS: usize = 1 << 16;

pub(crate) type Game = Arc<Mutex<Match>>;

pub(crate) fn lock(game: &Game) -> MutexGuard<'_, Match> {
    game.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Pause {
    /// Not paused by the host: if the call is pending, Wasmtime's fuel slice ran out.
    None,
    EndTurn,
    /// A host call could not be paid for and waits for the next turn.
    HostOutOfFuel,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Exit {
    ProcExit(i32),
}

/// Store data of one unit.
pub struct UnitData {
    pub(crate) unit: u32,
    game: Game,
    limits: StoreLimits,
    memory: Option<Memory>,
    log: Vec<u8>,
    log_limit: usize,
    log_truncated: bool,
    rng: u64,
    pause: Pause,
    pub(crate) exit: Option<Exit>,
    /// Odometer reading at which this turn's budget is used up.
    floor: u64,
    /// This turn's budget, before any debt is taken off it; 0 before the first turn.
    budget: u64,
    /// The stack height global added by `stack::instrument`, and the limit it counts against.
    stack_height: Option<Global>,
    stack_limit: u32,
}

impl UnitData {
    pub(crate) fn tick(&self) -> u32 {
        lock(&self.game).tick()
    }

    pub(crate) fn log_append(&mut self, bytes: &[u8]) {
        let room = self.log_limit.saturating_sub(self.log.len());
        let n = room.min(bytes.len());
        self.log.extend_from_slice(&bytes[..n]);
        if n < bytes.len() {
            self.log_truncated = true;
        }
    }

    /// splitmix64 stream seeded from the match seed and the unit id.
    pub(crate) fn fill_random(&mut self, out: &mut [u8]) {
        for chunk in out.chunks_mut(8) {
            self.rng = self.rng.wrapping_add(0x9e3779b97f4a7c15);
            let mut z = self.rng;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            z ^= z >> 31;
            chunk.copy_from_slice(&z.to_le_bytes()[..chunk.len()]);
        }
    }
}

fn range(len: usize, ptr: u32, n: u32) -> Option<std::ops::Range<usize>> {
    let start = ptr as usize;
    let end = start.checked_add(n as usize)?;
    (end <= len).then_some(start..end)
}

pub(crate) fn read_bytes(caller: &Caller<'_, UnitData>, ptr: u32, out: &mut [u8]) -> bool {
    let Some(mem) = caller.data().memory else { return false };
    let data = mem.data(caller);
    match range(data.len(), ptr, out.len() as u32) {
        Some(r) => {
            out.copy_from_slice(&data[r]);
            true
        }
        None => false,
    }
}

pub(crate) fn write_bytes(caller: &mut Caller<'_, UnitData>, ptr: u32, bytes: &[u8]) -> bool {
    let Some(mem) = caller.data().memory else { return false };
    let data = mem.data_mut(caller);
    match range(data.len(), ptr, bytes.len() as u32) {
        Some(r) => {
            data[r].copy_from_slice(bytes);
            true
        }
        None => false,
    }
}

/// Append guest bytes to the unit log (subject to the cap). False if the range is not in guest memory.
pub(crate) fn log_guest(caller: &mut Caller<'_, UnitData>, ptr: u32, len: u32, newline: bool) -> bool {
    let Some(mem) = caller.data().memory else { return false };
    let (bytes, data) = mem.data_and_store_mut(&mut *caller);
    let Some(r) = range(bytes.len(), ptr, len) else { return false };
    let src = &bytes[r];
    data.log_append(src);
    if newline && !src.ends_with(b"\n") {
        data.log_append(b"\n");
    }
    true
}

/// A future that is pending exactly once: suspends the bot's fiber until the driver polls it again.
struct YieldOnce(bool);

impl Future for YieldOnce {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            Poll::Pending
        }
    }
}

/// Fuel left in this turn's budget.
fn remaining(caller: &Caller<'_, UnitData>) -> wasmtime::Result<u64> {
    Ok(caller.get_fuel()?.saturating_sub(caller.data().floor))
}

/// Pay `cost` for a WASI call at once, as an instruction is paid: whatever this turn's budget cannot cover is
/// owed by the next turns, and the bot stops at its next fuel check. For calls that touch only the unit's own
/// memory and log, so nothing they do needs to wait for a turn that can pay for it.
pub(crate) fn charge_now(caller: &mut Caller<'_, UnitData>, cost: u64) -> wasmtime::Result<()> {
    let fuel = caller.get_fuel()?;
    caller.set_fuel(fuel.saturating_sub(cost))?;
    // Before the first turn (a wasm start function) there is no slice to keep.
    if caller.data().budget > 0 {
        // A slice of what is left, or of 1 fuel once past the floor: the next fuel check suspends the bot.
        let left = fuel.saturating_sub(caller.data().floor);
        caller.fuel_async_yield_interval(Some(left.saturating_sub(cost).max(1)))?;
    }
    Ok(())
}

/// Pay `cost` for a host call out of this turn's budget, waiting for a later turn if it cannot be paid.
/// A call that costs a whole turn's budget or more takes what is left and owes the rest, so no fee is too
/// large for a budget: it delays the call by as many turns as it costs.
async fn charge(caller: &mut Caller<'_, UnitData>, cost: u64) -> wasmtime::Result<()> {
    loop {
        let fuel = caller.get_fuel()?;
        let left = fuel.saturating_sub(caller.data().floor);
        if left > cost {
            // Move the odometer by `cost` and shrink the active slice to what is left, so that wasm still
            // suspends exactly at the floor. Never enlarges the budget: left - cost < left.
            caller.set_fuel(fuel - cost)?;
            caller.fuel_async_yield_interval(Some(left - cost))?;
            return Ok(());
        }
        caller.data_mut().pause = Pause::HostOutOfFuel;
        if cost >= caller.data().budget {
            // No turn of this budget could pay: move the odometer past the floor. The driver books the
            // overrun as debt, the next turns pay it off before they poll again, and the call goes through
            // on the first turn that has fuel left after it.
            caller.set_fuel(fuel.saturating_sub(cost))?;
            YieldOnce(false).await;
            return Ok(());
        }
        // Out of fuel inside a host call. The driver starts a new turn (new floor, new slice) before it
        // polls again; then the call is charged against that turn.
        YieldOnce(false).await;
    }
}

type C<'a> = Caller<'a, UnitData>;

async fn h_end_turn(mut caller: C<'_>) -> wasmtime::Result<()> {
    caller.data_mut().pause = Pause::EndTurn;
    YieldOnce(false).await;
    Ok(())
}

async fn h_count(mut caller: C<'_>, table: u32) -> wasmtime::Result<i32> {
    charge(&mut caller, ibc_abi::FUEL_COUNT).await?;
    let d = caller.data();
    Ok(lock(&d.game).count(d.unit, table))
}

async fn h_get(mut caller: C<'_>, table: u32, index: u32, out: u32, cap: u32) -> wasmtime::Result<i32> {
    charge(&mut caller, ibc_abi::FUEL_GET).await?;
    let mut row = Vec::new();
    let r = {
        let d = caller.data();
        lock(&d.game).get(d.unit, table, index, &mut row)
    };
    if r < 0 {
        return Ok(r);
    }
    // Records only grow at the end, so a bot built against a shorter record gets the prefix it knows.
    // A map row has no such prefix: it must fit.
    if row.len() > cap as usize && table == ibc_abi::T_MAP_ROW {
        return Ok(-ibc_abi::ERR_BUFFER_TOO_SMALL);
    }
    let n = row.len().min(cap as usize);
    if !write_bytes(&mut caller, out, &row[..n]) {
        return Ok(-ibc_abi::ERR_BAD_PTR);
    }
    Ok(n as i32)
}

async fn h_act(mut caller: C<'_>, kind: u32, a: i32, b: i32, c: i32) -> wasmtime::Result<i32> {
    charge(&mut caller, ibc_abi::FUEL_ACT).await?;
    let d = caller.data();
    Ok(lock(&d.game).act(d.unit, kind, a, b, c))
}

async fn h_los(mut caller: C<'_>, x0: i32, y0: i32, x1: i32, y1: i32) -> wasmtime::Result<i32> {
    charge(&mut caller, ibc_abi::FUEL_LOS).await?;
    Ok(lock(&caller.data().game).los(x0, y0, x1, y1))
}

async fn h_raycast(mut caller: C<'_>, x: i32, y: i32, angle: i32, max: i32) -> wasmtime::Result<i32> {
    charge(&mut caller, ibc_abi::FUEL_RAYCAST).await?;
    Ok(lock(&caller.data().game).raycast(x, y, angle, max))
}

async fn h_mem_read(mut caller: C<'_>, scope: u32, off: u32, ptr: u32, len: u32) -> wasmtime::Result<i32> {
    charge(&mut caller, ibc_abi::FUEL_MEM).await?;
    if len > MAX_MEM_IO {
        return Ok(ibc_abi::ERR_BAD_ARG);
    }
    let mut buf = vec![0u8; len as usize];
    let r = {
        let d = caller.data();
        lock(&d.game).mem_read(d.unit, scope, off, &mut buf)
    };
    if r != ibc_abi::OK {
        return Ok(r);
    }
    Ok(if write_bytes(&mut caller, ptr, &buf) { ibc_abi::OK } else { ibc_abi::ERR_BAD_PTR })
}

async fn h_mem_write(mut caller: C<'_>, scope: u32, off: u32, ptr: u32, len: u32) -> wasmtime::Result<i32> {
    charge(&mut caller, ibc_abi::FUEL_MEM).await?;
    if len > MAX_MEM_IO {
        return Ok(ibc_abi::ERR_BAD_ARG);
    }
    let mut buf = vec![0u8; len as usize];
    if !read_bytes(&caller, ptr, &mut buf) {
        return Ok(ibc_abi::ERR_BAD_PTR);
    }
    let d = caller.data();
    Ok(lock(&d.game).mem_write(d.unit, scope, off, &buf))
}

#[allow(clippy::too_many_arguments)]
async fn h_debug_draw(mut caller: C<'_>, kind: u32, x0: i32, y0: i32, x1: i32, y1: i32, color: u32) -> wasmtime::Result<i32> {
    charge(&mut caller, ibc_abi::FUEL_DEBUG).await?;
    let d = caller.data();
    Ok(lock(&d.game).debug_draw(d.unit, kind, x0, y0, x1, y1, color))
}

/// Every import of module `ibc` this host provides (ABI v0).
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

pub(crate) fn add_to_linker(l: &mut Linker<UnitData>) -> wasmtime::Result<()> {
    let m = ibc_abi::IMPORT_MODULE;
    l.func_wrap_async(m, "end_turn", |c: C<'_>, ()| Box::new(h_end_turn(c)))?;
    l.func_wrap(m, "fuel_remaining", |c: C<'_>| -> wasmtime::Result<i64> { Ok(remaining(&c)? as i64) })?;
    l.func_wrap_async(m, "count", |c: C<'_>, (t,): (u32,)| Box::new(h_count(c, t)))?;
    l.func_wrap_async(m, "get", |c: C<'_>, (t, i, o, cap): (u32, u32, u32, u32)| Box::new(h_get(c, t, i, o, cap)))?;
    l.func_wrap_async(m, "act", |c: C<'_>, (k, a, b, cc): (u32, i32, i32, i32)| Box::new(h_act(c, k, a, b, cc)))?;
    l.func_wrap_async(m, "los", |c: C<'_>, (a, b, x, y): (i32, i32, i32, i32)| Box::new(h_los(c, a, b, x, y)))?;
    l.func_wrap_async(m, "raycast", |c: C<'_>, (x, y, a, mx): (i32, i32, i32, i32)| Box::new(h_raycast(c, x, y, a, mx)))?;
    l.func_wrap_async(m, "mem_read", |c: C<'_>, (s, o, p, n): (u32, u32, u32, u32)| Box::new(h_mem_read(c, s, o, p, n)))?;
    l.func_wrap_async(m, "mem_write", |c: C<'_>, (s, o, p, n): (u32, u32, u32, u32)| Box::new(h_mem_write(c, s, o, p, n)))?;
    l.func_wrap_async(
        m,
        "debug_draw",
        |c: C<'_>, (k, x0, y0, x1, y1, col): (u32, i32, i32, i32, i32, u32)| Box::new(h_debug_draw(c, k, x0, y0, x1, y1, col)),
    )?;
    l.func_wrap(m, "log", |mut c: C<'_>, ptr: u32, len: u32| -> i32 {
        if log_guest(&mut c, ptr, len, true) {
            ibc_abi::OK
        } else {
            ibc_abi::ERR_BAD_PTR
        }
    })?;
    Ok(())
}

pub(crate) enum TurnEnd {
    /// The bot called `end_turn`.
    Yielded,
    /// The budget ran out (in wasm or in a host call); the bot resumes there next turn.
    OutOfFuel,
    /// The bot trapped, exited or returned from `_start`. The VM must not be run again.
    Fault(String),
}

pub(crate) struct Turn {
    pub used: u64,
    pub end: TurnEnd,
}

type StartCall = Pin<Box<dyn Future<Output = wasmtime::Result<()>>>>;

/// A unit's instance with its `_start` call in flight.
pub(crate) struct UnitVm {
    /// Heap-allocated store, owned by this struct (freed in `Drop`). A raw pointer because `call` holds a
    /// `&mut` to it for as long as the bot runs.
    store: NonNull<Store<UnitData>>,
    /// The suspended `_start` call. It holds a `&mut Store` derived from `store`; see `store_mut`.
    call: Option<StartCall>,
}

impl UnitVm {
    /// `module` must have been instrumented by `stack::instrument` with `stack_limit`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        engine: &Engine,
        linker: &Linker<UnitData>,
        module: &Module,
        game: Game,
        unit: u32,
        seed: u64,
        log_limit: usize,
        memory_limit: usize,
        stack_limit: u32,
    ) -> Result<UnitVm, String> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(memory_limit)
            .memories(1)
            .tables(1)
            .table_elements(MAX_TABLE_ELEMENTS)
            .instances(1)
            .build();
        let data = UnitData {
            unit,
            game,
            limits,
            memory: None,
            log: Vec::new(),
            log_limit,
            log_truncated: false,
            rng: seed ^ (unit as u64 + 1).wrapping_mul(0xd6e8feb86659fd93),
            pause: Pause::None,
            exit: None,
            floor: INITIAL_FUEL,
            budget: 0,
            stack_height: None,
            stack_limit,
        };
        let mut store = Box::new(Store::new(engine, data));
        store.limiter(|d| &mut d.limits);
        // Instantiation gets a small fixed allowance: Wasmtime refuses to instantiate a module with active
        // data segments on an empty tank, and a wasm start function (rare) may run this much before it traps.
        // The real odometer is set right after, so nothing here leaks into a turn budget.
        store.set_fuel(INSTANTIATE_FUEL).map_err(|e| e.to_string())?;
        let instance = {
            let mut fut = Box::pin(linker.instantiate_async(&mut *store, module));
            match fut.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
                Poll::Ready(r) => r.map_err(|e| format!("instantiation failed: {}", root(&e)))?,
                Poll::Pending => return Err("instantiation suspended (start function called end_turn?)".into()),
            }
        };
        let start = instance
            .get_typed_func::<(), ()>(&mut *store, "_start")
            .map_err(|e| format!("no usable `_start` export: {}", root(&e)))?;
        store.data_mut().memory = instance.get_memory(&mut *store, "memory");
        store.data_mut().stack_height = instance.get_global(&mut *store, crate::stack::HEIGHT_EXPORT);
        store.set_fuel(INITIAL_FUEL).map_err(|e| e.to_string())?;

        let store = NonNull::from(Box::leak(store));
        let call: StartCall = Box::pin(async move {
            // SAFETY: the store outlives this future (`Drop` drops the future first). This `&mut` stays live
            // while the call is suspended and `store_mut` aliases it between polls; see there.
            let store = unsafe { &mut *store.as_ptr() };
            start.call_async(store, ()).await
        });
        Ok(UnitVm { store, call: Some(call) })
    }

    /// The store, for use between polls of `call`.
    ///
    /// SAFETY argument. A suspended call keeps the `&mut Store` made in `new` alive, and so does Wasmtime
    /// (the `StoreContextMut` of `call_async`, the `&mut StoreOpaque` of its fiber future). The `&mut` made
    /// here aliases them. Rust's aliasing rules do not allow that, and Wasmtime 51 has no safe way to reach
    /// the store of a suspended async call (docs/TODO.md). It works because:
    ///
    /// * Access is on one thread and strictly alternates. The store is boxed and outlives `call`, and
    ///   `UnitVm` is neither `Send` nor `Sync`. `call` is polled only in `run_turn`, and a reference from
    ///   here cannot be held across that poll because both borrow `self` mutably. Between polls the call's
    ///   fiber is parked (`StoreFiberYield::KeepStore`) in one of our host calls or in Wasmtime's
    ///   `out_of_gas` libcall, and nothing runs on it.
    /// * Nothing parked on that fiber holds fuel or `UnitData` across the suspension. Our host calls read
    ///   both again through `Caller` when they resume. For the rest this relies on three things Wasmtime 51
    ///   does (re-check them on every bump; `vm::tests` fails if a change to one miscounts fuel):
    ///   1. Generated code keeps fuel in a local, stores it to `VMStoreContext::fuel_consumed` before every
    ///      call and libcall and reloads it after (`fuel_before_op`, `fuel_after_op`, `fuel_check` in
    ///      wasmtime-internal-cranelift's `func_environ.rs`). So `get_fuel` here counts all the wasm that
    ///      ran, and resumed code runs on the fuel set here.
    ///   2. `out_of_gas` (`runtime/vm/libcalls.rs`) refuels the slice from the reserve before `yield_now`
    ///      and returns straight to wasm after it. Resuming a fiber (`FiberResumeState`, `runtime/fiber.rs`)
    ///      restores TLS, MPK mask, stack limit, guard range and executor, not fuel.
    ///   3. `fuel_async_yield_interval` splits the fuel at once into an active slice of at most the interval
    ///      and a reserve (`set_fuel(get_fuel())` in `runtime/store.rs`), so the next poll suspends at the
    ///      first fuel check at or past the new floor.
    fn store_mut(&mut self) -> &mut Store<UnitData> {
        // SAFETY: see above.
        unsafe { &mut *self.store.as_ptr() }
    }

    /// Run the bot for one turn with `budget` fuel.
    pub(crate) fn run_turn(&mut self, budget: u64) -> Turn {
        let store = self.store_mut();
        let start = store.get_fuel().expect("fuel is enabled");
        // Fuel used beyond the previous floor is owed now.
        let debt = store.data().floor.saturating_sub(start);
        if debt >= budget {
            // Nothing left to run with: the whole turn pays off debt.
            store.data_mut().floor = start + (debt - budget);
            return Turn { used: 0, end: TurnEnd::OutOfFuel };
        }
        let allowance = budget - debt;
        store.data_mut().floor = start - allowance;
        store.data_mut().budget = budget;
        store.data_mut().pause = Pause::None;
        // Arms the active slice to exactly `allowance` (the reserve is far larger).
        store.fuel_async_yield_interval(Some(allowance)).expect("fuel is enabled");

        let Some(call) = self.call.as_mut() else {
            return Turn { used: 0, end: TurnEnd::Fault("bot is not running".into()) };
        };
        let polled = call.as_mut().poll(&mut Context::from_waker(Waker::noop()));

        let store = self.store_mut();
        let used = start.saturating_sub(store.get_fuel().expect("fuel is enabled"));
        let exit = store.data().exit;
        let end = match polled {
            Poll::Pending => match store.data().pause {
                Pause::EndTurn => TurnEnd::Yielded,
                Pause::HostOutOfFuel | Pause::None => TurnEnd::OutOfFuel,
            },
            Poll::Ready(Ok(())) => TurnEnd::Fault("`_start` returned".into()),
            Poll::Ready(Err(e)) => TurnEnd::Fault(match exit {
                Some(Exit::ProcExit(code)) => format!("exit({code})"),
                None => match e.downcast_ref::<Trap>() {
                    Some(t) => trap_reason(store, *t),
                    None if e.is::<ThrownException>() => "uncaught wasm exception".into(),
                    None => format!("error: {}", root(&e)),
                },
            }),
        };
        if matches!(end, TurnEnd::Fault(_)) {
            self.call = None;
        }
        Turn { used, end }
    }

    /// The log from byte `from` on (the log stops growing at its cap).
    pub(crate) fn log_since(&mut self, from: usize) -> Vec<u8> {
        self.store_mut().data().log.get(from..).unwrap_or_default().to_vec()
    }

    /// Stop the bot and return its log.
    pub(crate) fn finish(mut self, note: Option<&str>) -> Vec<u8> {
        self.call = None;
        let data = self.store_mut().data_mut();
        let mut log = std::mem::take(&mut data.log);
        if data.log_truncated {
            log.extend_from_slice(format!("\n[ibc] log truncated at {} bytes\n", data.log_limit).as_bytes());
        }
        if let Some(note) = note {
            if !log.is_empty() && !log.ends_with(b"\n") {
                log.push(b'\n');
            }
            log.extend_from_slice(note.as_bytes());
            log.push(b'\n');
        }
        log
    }
}

impl Drop for UnitVm {
    fn drop(&mut self) {
        // The call borrows the store: drop it first (this unwinds the suspended fiber).
        self.call = None;
        // SAFETY: allocated by `Box::new` in `new`, leaked once, freed once here.
        drop(unsafe { Box::from_raw(self.store.as_ptr()) });
    }
}

fn root(e: &wasmtime::Error) -> String {
    e.root_cause().to_string()
}

/// Why a bot trapped, in a player's words. The stack limiter traps with `unreachable` after raising the
/// height past the limit, and a frame that passed the check never leaves it there, so that pair is the
/// limiter's trap and nothing else.
fn trap_reason(store: &mut Store<UnitData>, trap: Trap) -> String {
    let (height, limit) = (store.data().stack_height, store.data().stack_limit);
    match height.map(|g| g.get(&mut *store)) {
        Some(Val::I32(h)) if trap == Trap::UnreachableCodeReached && h as u32 > limit => {
            format!("trap: stack limit exceeded (deeper than stack_limit = {limit} units)")
        }
        _ => format!("trap: {trap}"),
    }
}

#[cfg(test)]
mod tests;
