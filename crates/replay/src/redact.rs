//! Views of a replay that hide a team's private data: its memory writes, its memory hashes and its records
//! (logs, per-turn fuel, debug drawings). Actions, faults and the world hash stay, so every view re-simulates
//! to the same world and still verifies.

use crate::{Event, Replay};
use ibc_engine::Input;

/// Which team's private data a view keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    /// Both teams (the full record).
    Both,
    /// Team 0 (A) or 1 (B) only.
    Team(usize),
    /// Neither team (the public view).
    None,
}

impl Keep {
    fn keeps(self, team: usize) -> bool {
        match self {
            Keep::Both => true,
            Keep::Team(t) => t == team,
            Keep::None => false,
        }
    }
}

impl Replay {
    /// Remove one team's private data. Idempotent.
    pub fn redact(&mut self, team: usize) {
        let team = team & 1;
        let h = &self.header;
        let owner = |unit: u32| h.unit_team(unit);
        for t in &mut self.ticks {
            t.events.retain(|e| !matches!(e, Event::Input(Input::MemWrite { unit, .. }) if owner(*unit) == team));
            t.hashes.memory[team] = 0;
            t.private[team].clear();
        }
        self.header.redacted[team] = true;
    }

    /// A copy with only the private data of `keep`.
    pub fn view(&self, keep: Keep) -> Replay {
        let mut r = self.clone();
        for team in (0..2).filter(|&t| !keep.keeps(t)) {
            r.redact(team);
        }
        r
    }
}
