//! Drive a match the way a test does, and keep a frame after every step.

use crate::record::{self, Catalog, Frame, Meta};
use ibc_engine::abi::Host;
use ibc_engine::Match;

pub struct Take {
    pub m: Match,
    frames: Vec<Frame>,
}

impl Take {
    pub fn begin(m: Match) -> Take {
        let frames = vec![record::snap(&m)];
        Take { m, frames }
    }

    pub fn act(&mut self, unit: u32, kind: u32, a: i32, b: i32) -> i32 {
        self.m.act(unit, kind, a, b, 0)
    }

    pub fn step(&mut self) {
        self.m.step();
        self.frames.push(record::snap(&self.m));
    }

    /// A change the test makes between ticks (damage, a teleport into reach).
    pub fn then(&mut self, f: impl FnOnce(&mut Match)) {
        f(&mut self.m);
        self.frames.push(record::snap(&self.m));
    }

    pub fn shots(&self, u: usize) -> i32 {
        self.m.world.units[u].stats.shots
    }

    pub fn finish(self, cat: &mut Catalog, meta: Meta) {
        cat.add(&self.m, meta, self.frames);
    }
}

pub fn meta(group: &str, id: &str, title: &str, note: &str, outcome: &str) -> Meta {
    Meta {
        id: id.to_string(),
        group: group.to_string(),
        title: title.to_string(),
        note: note.to_string(),
        outcome: outcome.to_string(),
        ttk: None,
    }
}
