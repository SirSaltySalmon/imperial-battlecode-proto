//! The smallest possible bot: it never acts. Useful as a punching bag and as a template.
use ibc::Bot;

pub struct Idle;

impl Bot for Idle {
    fn new() -> Self {
        Idle
    }

    fn turn(&mut self) {}
}
