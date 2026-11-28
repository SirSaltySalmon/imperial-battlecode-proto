//! A play session: the state the web page and the match thread share, and the `Driver` that lets the player
//! take over one unit of their team from its bot.
//!
//! The match runs on its own thread inside `ibc_runner::run_driven`. Every unit of the player's team runs the
//! team's bot, except the one the player possesses: its turn is taken here, at its place in the turn order,
//! with the acts the `Controller` makes from the latest input. The draft is the bot's, except a kit chosen
//! up front with `--kit`, which the session drafts for that unit at its turn. Between ticks
//! the driver's `observe` publishes the team's view and then waits: while the player acts (active mode), for
//! the tick clock (real-time mode) or for a `/step` (turn mode). Wall-clock time only decides *when* a tick
//! runs, never what is in it, so the replay (the acts as they were made) re-simulates exactly.

use super::input::{act_name, code_name, Act, Controller, Input};
use super::state::{self, TeamView};
use ibc_abi::*;
use ibc_engine::Match;
use serde_json::{json, Value};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Time passes at `tps` while the player acts (moves, fires, holds Space) and stops when they do not.
    Active { tps: u32 },
    /// Ticks run on a clock; the latest input is used each tick.
    Realtime { tps: u32 },
    /// One tick per `/step`.
    Turn,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Active { .. } => "active",
            Mode::Realtime { .. } => "realtime",
            Mode::Turn => "turn",
        }
    }

    /// Ticks per second of the clock, 0 in turn mode.
    pub fn tps(self) -> u32 {
        match self {
            Mode::Active { tps } | Mode::Realtime { tps } => tps,
            Mode::Turn => 0,
        }
    }
}

/// The player's team.
#[derive(Clone, Copy, Debug)]
pub struct Seat {
    pub team: i32,
    pub team_size: u32,
}

impl Seat {
    /// The unit id of a slot of the team.
    pub fn unit(self, slot: u32) -> u32 {
        self.team as u32 * self.team_size + slot
    }
}

struct Shared {
    controller: Controller,
    /// The slot of the unit the player drives, if any; the others run the team's bot.
    possessed: Option<u32>,
    /// The clock is running on its own: real-time mode, or active mode with time let run (P).
    running: bool,
    /// Ticks asked for with `/step` and not run yet.
    steps: u32,
    quit: bool,
    /// The team's view after the latest tick, that tick, its phase and which of the team's units are alive.
    view: TeamView,
    tick: u32,
    phase: i32,
    alive: Vec<bool>,
    /// The player's acts of the latest tick with the engine's answers.
    last: Vec<(Act, i32)>,
    /// Set when the match is over: result line and where the replay was written.
    done: Option<Value>,
}

impl Shared {
    fn publish(&mut self, game: &Match, seat: Seat) {
        self.view = state::team_view(game, seat.team);
        self.tick = game.tick();
        self.phase = game.world.phase;
        self.alive = (0..seat.team_size)
            .map(|s| game.world.units[seat.unit(s) as usize].alive)
            .collect();
    }

    /// Active mode: whether the next tick should run. The draft (made by the bots) and the pause after a round
    /// need no decision from the player and run on their own; the rest waits for the player to act.
    fn wants_time(&self) -> bool {
        let alive = self
            .possessed
            .is_some_and(|p| self.alive.get(p as usize).copied().unwrap_or(false));
        matches!(self.phase, PHASE_DRAFT | PHASE_ROUND_END)
            || self.controller.busy(self.phase, alive)
    }
}

pub struct Session {
    pub mode: Mode,
    pub seat: Seat,
    /// Static facts for the page (map, rules, names), served as `/setup`.
    pub setup: Value,
    /// (unit, kit) drafted in place of the team bot's pick.
    draft: Option<(u32, i32)>,
    shared: Mutex<Shared>,
    changed: Condvar,
}

/// How long `/step` waits for its tick before answering with whatever state there is.
const STEP_WAIT: Duration = Duration::from_secs(10);
/// How long `GET /state?after=N` waits for a newer tick.
const POLL_WAIT: Duration = Duration::from_secs(2);

impl Session {
    /// `start` is the match before the first tick; `possessed` the slot the player starts in, if any;
    /// `draft` a (unit, kit) the session drafts instead of the team's bot.
    pub fn new(
        mode: Mode,
        seat: Seat,
        setup: Value,
        start: &Match,
        possessed: Option<u32>,
        draft: Option<(u32, i32)>,
    ) -> Arc<Session> {
        let mut shared = Shared {
            controller: Controller::default(),
            possessed,
            running: false,
            steps: 0,
            quit: false,
            view: state::team_view(start, seat.team),
            tick: 0,
            phase: PHASE_DRAFT,
            alive: Vec::new(),
            last: Vec::new(),
            done: None,
        };
        shared.publish(start, seat);
        Arc::new(Session {
            mode,
            seat,
            setup,
            draft,
            shared: Mutex::new(shared),
            changed: Condvar::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn input(&self, input: Input) {
        self.lock().controller.apply(input);
        // In active mode an input can start time.
        self.changed.notify_all();
    }

    /// Take over the unit in `slot` from the team's bot (`None`: give it back and watch). Queued actions
    /// were for the previous unit and are dropped.
    pub fn possess(&self, slot: Option<u32>) -> Result<(), String> {
        if let Some(n) = slot.filter(|&n| n >= self.seat.team_size) {
            return Err(format!(
                "there is no unit {n}: your team has units 0..{}",
                self.seat.team_size - 1
            ));
        }
        let mut s = self.lock();
        if s.possessed != slot {
            s.controller.clear_queue();
            s.last.clear();
            s.possessed = slot;
        }
        self.changed.notify_all();
        Ok(())
    }

    /// Ask for one tick (applying `input` first) and wait until it has run; returns the state after it.
    pub fn step(&self, input: Option<Input>) -> Value {
        let mut s = self.lock();
        if let Some(i) = input {
            s.controller.apply(i);
        }
        let target = s.tick + s.steps + 1;
        s.steps += 1;
        self.changed.notify_all();
        let deadline = Instant::now() + STEP_WAIT;
        while s.tick < target && s.done.is_none() && !s.quit {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else { break };
            s = self.changed.wait_timeout(s, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        drop(s);
        self.state()
    }

    /// Let the clock run on its own, or stop it. Held controls and queued actions stay as they are: the page
    /// owns them and keeps sending changes.
    pub fn set_running(&self, running: bool) {
        self.lock().running = running;
        self.changed.notify_all();
    }

    /// Stop the match at the current tick (it is recorded as unfinished) and end the session.
    pub fn quit(&self) {
        self.lock().quit = true;
        self.changed.notify_all();
    }

    pub fn quitting(&self) -> bool {
        self.lock().quit
    }

    /// The match is over and its files are written.
    pub fn finish(&self, done: Value) {
        self.lock().done = Some(done);
        self.changed.notify_all();
    }

    /// Block until `/quit`.
    pub fn wait_quit(&self) {
        let mut s = self.lock();
        while !s.quit {
            s = self.changed.wait(s).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// What `/state?after=N` answers: the state as soon as a tick after `after` has run (or the match is
    /// over), or as it is after a short wait. Lets the page see each tick when it happens, without polling.
    pub fn state_after(&self, after: u32) -> Value {
        let deadline = Instant::now() + POLL_WAIT;
        let mut s = self.lock();
        while s.tick <= after && s.done.is_none() && !s.quit {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            s = self
                .changed
                .wait_timeout(s, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        drop(s);
        self.state()
    }

    /// What `/state` answers.
    pub fn state(&self) -> Value {
        let s = self.lock();
        let last: Vec<Value> = s
            .last
            .iter()
            .map(|(a, code)| json!({ "act": act_name(a.kind), "a": a.a, "code": code, "result": code_name(*code) }))
            .collect();
        json!({
            "mode": self.mode.name(),
            "running": s.running,
            "tick": s.tick,
            "possessed": s.possessed,
            "queued": s.controller.queued(),
            "last_acts": last,
            "done": s.done,
            "view": s.view.view(s.possessed),
        })
    }

    /// The `Driver` for the runner. One per session.
    pub fn driver(self: &Arc<Session>) -> PlayDriver {
        PlayDriver {
            session: Arc::clone(self),
            last_tick: None,
        }
    }
}

pub struct PlayDriver {
    session: Arc<Session>,
    /// When the latest tick ran, for the clock of the active and real-time modes.
    last_tick: Option<Instant>,
}

impl PlayDriver {
    /// Active and real-time modes: wait until the next tick is due (or a `/step`). In real-time mode the
    /// clock runs while `running`; in active mode also while the player wants time. False on quit.
    fn wait_clock(&mut self, tps: u32, active: bool) -> bool {
        let period = Duration::from_secs(1) / tps.max(1);
        let session = Arc::clone(&self.session);
        let mut s = session.lock();
        loop {
            if s.quit {
                return false;
            }
            let now = Instant::now();
            if s.steps > 0 {
                s.steps -= 1;
                self.last_tick = Some(now);
                return true;
            }
            if !(s.running || active && s.wants_time()) {
                s = session.changed.wait(s).unwrap_or_else(|e| e.into_inner());
                continue;
            }
            // Time that starts again runs at once, but never faster than `tps`.
            let due = self.last_tick.map_or(now, |t| t + period);
            if now >= due {
                // A late tick does not make the next ones hurry to catch up.
                self.last_tick = Some(if now - due > period { now } else { due });
                return true;
            }
            s = session.changed.wait_timeout(s, due - now).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// Turn mode: wait for a `/step`. False on quit.
    fn wait_step(&mut self) -> bool {
        let session = &self.session;
        let mut s = session.lock();
        loop {
            if s.quit {
                return false;
            }
            if s.steps > 0 {
                s.steps -= 1;
                return true;
            }
            s = session.changed.wait(s).unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl ibc_runner::Driver for PlayDriver {
    fn drives(&self, _unit: u32, _team: i32) -> bool {
        // Every unit can run the team's bot: the player only borrows one at a time.
        false
    }

    fn observe(&mut self, game: &Match) -> bool {
        {
            let mut s = self.session.lock();
            s.publish(game, self.session.seat);
            self.session.changed.notify_all();
        }
        if game.finished() {
            return true;
        }
        match self.session.mode {
            Mode::Turn => self.wait_step(),
            Mode::Realtime { tps } => self.wait_clock(tps, false),
            Mode::Active { tps } => self.wait_clock(tps, true),
        }
    }

    fn turn(&mut self, unit: u32, game: &mut Match) -> bool {
        let seat = self.session.seat;
        let mut s = self.session.lock();
        if game.world.phase == PHASE_DRAFT {
            let Some((_, kit)) = self.session.draft.filter(|&(u, _)| u == unit) else {
                return false;
            };
            let draft = Act {
                kind: A_DRAFT,
                a: kit,
                b: 0,
                c: 0,
            };
            s.last = vec![(draft, game.act(unit, A_DRAFT, kit, 0, 0))];
            return true;
        }
        if s.possessed.map(|slot| seat.unit(slot)) != Some(unit) {
            return false;
        }
        let acts = s.controller.acts(&*game, unit);
        s.last = acts
            .into_iter()
            .map(|a| (a, game.act(unit, a.kind, a.a, a.b, a.c)))
            .collect();
        true
    }
}
