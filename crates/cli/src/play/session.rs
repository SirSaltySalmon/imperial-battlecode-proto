//! A play session: the state the web page and the match thread share, and the `Driver` that puts the
//! player's unit into the runner's turn order.
//!
//! The match runs on its own thread inside `ibc_runner::run_driven`. Between ticks the driver's `observe`
//! publishes the page's view and then waits: for a `/step` in turn mode, for the tick clock in real-time
//! mode. The player's unit takes its turn at its place in the turn order like any bot, with the acts the
//! `Controller` makes from the latest input. Wall-clock time only decides *when* a tick runs, never what is
//! in it, so the replay (the acts as they were made) re-simulates exactly.

use super::input::{act_name, code_name, Act, Controller, Input};
use super::state;
use ibc_abi::Host;
use ibc_engine::Match;
use serde_json::{json, Value};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Ticks run on a clock; the latest input is used each tick.
    Realtime { tps: u32 },
    /// One tick per `/step`.
    Turn,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Realtime { .. } => "realtime",
            Mode::Turn => "turn",
        }
    }
}

/// Who the player is.
#[derive(Clone, Copy, Debug)]
pub struct Seat {
    pub unit: u32,
    pub team: i32,
    /// The player's teammates run a bot (`--allies`) rather than standing still.
    pub allies: bool,
}

struct Shared {
    controller: Controller,
    /// Real-time mode: the clock is running (it starts paused, so the player can draft and look around).
    running: bool,
    /// Ticks asked for with `/step` and not run yet.
    steps: u32,
    quit: bool,
    /// The page's view after the latest tick, and that tick.
    view: Value,
    tick: u32,
    /// The player's acts of the latest tick with the engine's answers.
    last: Vec<(Act, i32)>,
    /// Set when the match is over: result line and where the replay was written.
    done: Option<Value>,
}

pub struct Session {
    pub mode: Mode,
    pub seat: Seat,
    /// Static facts for the page (map, rules, names), served as `/setup`.
    pub setup: Value,
    shared: Mutex<Shared>,
    changed: Condvar,
}

/// How long `/step` waits for its tick before answering with whatever state there is.
const STEP_WAIT: Duration = Duration::from_secs(10);

impl Session {
    /// `view` is the page's view before the first tick.
    pub fn new(mode: Mode, seat: Seat, setup: Value, view: Value) -> Arc<Session> {
        Arc::new(Session {
            mode,
            seat,
            setup,
            shared: Mutex::new(Shared {
                controller: Controller::default(),
                running: false,
                steps: 0,
                quit: false,
                view,
                tick: 0,
                last: Vec::new(),
                done: None,
            }),
            changed: Condvar::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn input(&self, input: Input) {
        self.lock().controller.apply(input);
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

    /// Start or pause the real-time clock. Held controls and queued actions stay as they are: the page owns
    /// them and keeps sending changes.
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
            "queued": s.controller.queued(),
            "last_acts": last,
            "done": s.done,
            "view": s.view,
        })
    }

    /// The `Driver` for the runner. One per session.
    pub fn driver(self: &Arc<Session>) -> PlayDriver {
        PlayDriver { session: Arc::clone(self), next_tick: None }
    }
}

pub struct PlayDriver {
    session: Arc<Session>,
    /// Real-time mode: when the next tick is due.
    next_tick: Option<Instant>,
}

impl PlayDriver {
    /// Real-time mode: wait for the clock (or a `/step` while paused). False on quit.
    fn wait_clock(&mut self, tps: u32) -> bool {
        let period = Duration::from_secs(1) / tps.max(1);
        let session = &self.session;
        let mut s = session.lock();
        loop {
            if s.quit {
                return false;
            }
            if s.steps > 0 {
                s.steps -= 1;
                self.next_tick = None;
                return true;
            }
            if !s.running {
                self.next_tick = None;
                s = session.changed.wait(s).unwrap_or_else(|e| e.into_inner());
                continue;
            }
            let now = Instant::now();
            let due = *self.next_tick.get_or_insert(now + period);
            if now >= due {
                // A late tick does not make the next ones hurry to catch up.
                self.next_tick = Some(if now - due > period { now + period } else { due + period });
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
    fn drives(&self, unit: u32, team: i32) -> bool {
        let seat = self.session.seat;
        team == seat.team && (unit == seat.unit || !seat.allies)
    }

    fn observe(&mut self, game: &Match) -> bool {
        let view = state::view(game, self.session.seat.unit);
        {
            let mut s = self.session.lock();
            s.view = view;
            s.tick = game.tick();
            self.session.changed.notify_all();
        }
        if game.finished() {
            return true;
        }
        match self.session.mode {
            Mode::Turn => self.wait_step(),
            Mode::Realtime { tps } => self.wait_clock(tps),
        }
    }

    fn turn(&mut self, unit: u32, game: &mut Match) {
        // Teammates without an allies bot stand still: they make no acts at all.
        if unit != self.session.seat.unit {
            return;
        }
        let mut s = self.session.lock();
        let acts = s.controller.acts(&*game, unit);
        s.last = acts.into_iter().map(|a| (a, game.act(unit, a.kind, a.a, a.b, a.c))).collect();
    }
}
