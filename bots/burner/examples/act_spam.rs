//! Does nothing and burns every tick's fuel on `A_MOVE` at speed 0, a new angle each call. In the action
//! phases every call is accepted until the unit reaches `MAX_ACTS` for the tick; the rest are refused.
use core::hint::black_box;

struct ActSpam(i32);

impl burner::Kernel for ActSpam {
    fn new() -> Self {
        ActSpam(0)
    }

    fn run(&mut self, n: u32) {
        for _ in 0..n {
            self.0 = (self.0 + 4099) & 0xffff;
            black_box(ibc::act(ibc::A_MOVE, self.0, 0, 0).is_ok());
        }
    }
}

ibc::bot!(burner::Burner<burner::Idle, ActSpam>);
