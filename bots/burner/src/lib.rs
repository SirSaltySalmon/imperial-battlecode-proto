//! Burner: spends the whole fuel budget of every tick. A load generator, not a strategy.
//!
//! `Burner<B, K>` plays bot `B`'s turn, then spends what is left of the tick's budget on kernel `K`, and
//! ends the turn about `RESERVE` fuel short of zero. `src/main.rs` is `Burner<Idle, Alu>`: nobody dies, every
//! round runs to its clock, and all ten units use their whole budget on all 2,305 ticks of the 18 rounds.
//! (Only a match whose every round is planted at the last moment is longer: 3,745 ticks.)
//!
//! Fuel is per unit and per tick (`crates/runner/src/vm.rs`). A unit that reaches the end of its budget is
//! suspended where it is and resumes there on its next tick; unused fuel is lost. The burner yields before
//! that happens rather than letting the meter stop it, so a strategy wrapped in it never loses a turn.
//!
//! The kernels spend the same fuel in different ways, because one unit of fuel does not cost the same CPU
//! time everywhere: an L1-resident multiply, a division, a load that misses every cache, a host call.
//! Natively (`ibc-sim`) there is no meter (`fuel_remaining()` is `i64::MAX`) and the burner burns nothing.
use core::hint::black_box;
use ibc::Bot;
use squad::{Attack, Buy, Defence, Squad, Style};

/// Fuel left unspent at the end of a turn: the path from the last check to `end_turn` fits in it.
const RESERVE: i64 = 256;

/// One way to spend fuel.
pub trait Kernel {
    fn new() -> Self;
    /// Called once per turn, before the first step.
    fn start(&mut self) {}
    /// `n` steps of work.
    fn run(&mut self, n: u32);
}

/// Does nothing (as `bots/idle`).
pub struct Idle;

impl Bot for Idle {
    fn new() -> Self {
        Idle
    }

    fn turn(&mut self) {}
}

/// `bots/forcer`: the strongest bot of the reference pool, so a real strategy to burn beside.
pub struct Forcer(Squad);

const FORCER: Style = Style { attack: Attack::Execute, defence: Defence::Push, buy: Buy::Force, mixed_arms: false };

impl Bot for Forcer {
    fn new() -> Self {
        Forcer(Squad::new(FORCER))
    }

    fn turn(&mut self) {
        self.0.turn()
    }
}

pub struct Burner<B, K> {
    bot: B,
    kernel: K,
}

impl<B: Bot, K: Kernel> Bot for Burner<B, K> {
    fn new() -> Self {
        Burner { bot: B::new(), kernel: K::new() }
    }

    fn turn(&mut self) {
        self.bot.turn();
        burn(&mut self.kernel);
    }
}

/// Spend this turn's fuel down to `RESERVE`. Each batch is sized from the measured cost of the previous one
/// and takes half of what is left, so no batch runs past the floor unless a step suddenly costs twice what
/// the steps before it did. About log2(budget / step) fuel checks per turn.
pub fn burn<K: Kernel>(kernel: &mut K) {
    if ibc::fuel_remaining() == i64::MAX {
        return;
    }
    kernel.start();
    let mut left = ibc::fuel_remaining();
    let mut n: u32 = 1;
    while left > RESERVE {
        kernel.run(black_box(n));
        let now = ibc::fuel_remaining();
        let per_step = ((left - now) / n as i64).max(1);
        left = now;
        let fit = (left - RESERVE) / per_step;
        if fit < 2 {
            break;
        }
        n = (fit / 2).min(u32::MAX as i64) as u32;
    }
}

/// A dependent multiply-rotate chain on registers: ordinary bot arithmetic.
pub struct Alu(u64);

impl Kernel for Alu {
    fn new() -> Self {
        Alu(0x9e37_79b9_7f4a_7c15)
    }

    fn run(&mut self, n: u32) {
        let mut x = self.0;
        for _ in 0..n {
            x = x.rotate_left(17) ^ x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        }
        self.0 = black_box(x);
    }
}

/// A dependent chain of 64-bit divisions by an unknown divisor: the slowest plain instruction per fuel.
pub struct Div {
    x: u64,
    d: u64,
}

impl Kernel for Div {
    fn new() -> Self {
        Div { x: u64::MAX, d: black_box(3) }
    }

    fn run(&mut self, n: u32) {
        let (mut x, d) = (self.x, self.d);
        for _ in 0..n {
            x = (x / d) ^ 0xf000_0000_0000_0000;
        }
        self.x = black_box(x);
    }
}

/// 16 MiB of `u32`: ten units hold 160 MiB, several times any last-level cache.
const CHASE_WORDS: usize = 1 << 22;

/// Dependent loads at pseudo-random places in a 16 MiB buffer: each step waits for a cache (and TLB) miss.
/// Allocating the buffer costs 16 Mi fuel up front (`memory.fill` is charged per byte), repaid over the
/// following turns like any overrun.
pub struct Chase {
    buf: Box<[u32; CHASE_WORDS]>,
    x: u32,
}

impl Kernel for Chase {
    fn new() -> Self {
        let mut buf: Box<[u32; CHASE_WORDS]> = vec![0u32; CHASE_WORDS].into_boxed_slice().try_into().unwrap();
        // Touch every 4 KiB page so that each has its own physical frame.
        for i in (0..CHASE_WORDS).step_by(1024) {
            buf[i] = i as u32;
        }
        Chase { buf, x: 1 }
    }

    fn run(&mut self, n: u32) {
        let mut x = self.x;
        for _ in 0..n {
            // The next address depends on the value just loaded, so the loads cannot overlap.
            let v = self.buf[(x >> 10) as usize & (CHASE_WORDS - 1)];
            x = (x ^ v).wrapping_mul(0x9e37_79b1).wrapping_add(0x7f4a_7c15);
        }
        self.x = black_box(x);
    }
}

/// `fuel_remaining()` in a loop: a host call that charges nothing on top of its own instructions.
pub struct FuelCalls;

impl Kernel for FuelCalls {
    fn new() -> Self {
        FuelCalls
    }

    fn run(&mut self, n: u32) {
        for _ in 0..n {
            black_box(ibc::fuel_remaining());
        }
    }
}

/// `debug_draw` in a loop: free (`FUEL_DEBUG` = 0), kept up to 4096 per tick and stored in the replay.
pub struct Draws(i32);

impl Kernel for Draws {
    fn new() -> Self {
        Draws(0)
    }

    fn run(&mut self, n: u32) {
        for _ in 0..n {
            self.0 = self.0.wrapping_add(1);
            ibc::debug_dot((self.0 & 0xffff, self.0 >> 16), 0xff00ff);
        }
    }
}

/// `los` from this unit to points round the edge of the map (`FUEL_LOS` = 60).
pub struct Los {
    from: (i32, i32),
    size: (i32, i32),
    i: i32,
}

impl Kernel for Los {
    fn new() -> Self {
        let m = ibc::map_info();
        Los { from: (0, 0), size: (m.width * ibc::TILE, m.height * ibc::TILE), i: 0 }
    }

    fn start(&mut self) {
        let me = ibc::me();
        self.from = (me.x, me.y);
    }

    fn run(&mut self, n: u32) {
        let (w, h) = self.size;
        for _ in 0..n {
            self.i = (self.i + 1) & 1023;
            let to = if self.i < 512 { (w * self.i / 512, h - 1) } else { (w - 1, h * (self.i - 512) / 512) };
            black_box(ibc::los(self.from, to));
        }
    }
}

/// `count(T_ENEMIES)` and `get(T_SELF)`: table reads (`FUEL_COUNT` = 10, `FUEL_GET` = 40).
pub struct Reads;

impl Kernel for Reads {
    fn new() -> Self {
        Reads
    }

    fn run(&mut self, n: u32) {
        for _ in 0..n {
            black_box(ibc::enemies().total());
            black_box(ibc::me().hp);
        }
    }
}

/// A 1 MiB `mem_read`, refused by the engine: `FUEL_MEM` = 20 whatever the length.
pub struct MemRead(Vec<u8>);

/// Length of the `MemRead` and `MemWrite` requests: 1 MiB, the most the runner passed on before it stopped
/// allocating a buffer of the requested length.
const MAX_MEM_IO: usize = 1 << 20;

impl Kernel for MemRead {
    fn new() -> Self {
        MemRead(vec![0; MAX_MEM_IO])
    }

    fn run(&mut self, n: u32) {
        for _ in 0..n {
            black_box(ibc::mem_read(ibc::MEM_TEAM, 0, &mut self.0).is_ok());
        }
    }
}

/// One refused 1 MiB `mem_write` per turn, then `Alu` for the rest. Kept to one call per turn so that it can
/// still be compared with engines that recorded every `mem_write`, refused or not, where a loop of them
/// exhausts memory.
pub struct MemWrite(Vec<u8>, Alu);

impl Kernel for MemWrite {
    fn new() -> Self {
        MemWrite(vec![0x5a; MAX_MEM_IO], Alu::new())
    }

    fn start(&mut self) {
        black_box(ibc::mem_write(ibc::MEM_TEAM, 0, &self.0).is_ok());
    }

    fn run(&mut self, n: u32) {
        self.1.run(n)
    }
}
