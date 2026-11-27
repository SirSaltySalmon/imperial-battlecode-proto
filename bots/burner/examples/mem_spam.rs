//! Does nothing and burns every tick's fuel on 64-byte `MEM_TEAM` writes, each different from the last. They
//! are accepted in the action phases: the replay keeps one write per team and tick, however many calls.
use core::hint::black_box;

struct MemSpam([u8; 64]);

impl burner::Kernel for MemSpam {
    fn new() -> Self {
        MemSpam([0; 64])
    }

    fn run(&mut self, n: u32) {
        for _ in 0..n {
            self.0[0] = self.0[0].wrapping_add(1);
            black_box(ibc::mem_write(ibc::MEM_TEAM, 0, &self.0).is_ok());
        }
    }
}

ibc::bot!(burner::Burner<burner::Idle, MemSpam>);
