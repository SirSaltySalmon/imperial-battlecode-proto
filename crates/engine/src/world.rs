//! Match state and tick resolution. CORE FILE: the combat, abilities and economy modules plug into the seams
//! called from here; they should not need to change this file.
//!
//! One tick (`step`), in the action phases:
//!   1. intents -> facing, primary actions (fire/reload/plant/defuse/ability/...), velocity
//!   2. `combat::pre_step`: weapon timers, wind-ups finishing release bullets
//!   3. `substeps` times: units move (collision), projectiles move (swept hits); damage applied per substep
//!   4. `abilities::step`: effects tick
//!   5. objective: plant/defuse channels, fuse, round end
//!   6. sounds, vision for the next tick
//! All units' intents come from the same start-of-tick snapshot, so nobody acts "first".

use crate::fixed::{self, V2};
use crate::vision::TeamView;
use crate::{abilities, combat, economy, Map, Ruleset};
use ibc_abi::*;

#[derive(Clone, Debug, Default)]
pub struct Intent {
    /// (angle, speed)
    pub mv: Option<(i32, i32)>,
    pub face: Option<i32>,
    /// (kind, a, b, c) of the last primary action queued this tick.
    pub primary: Option<(u32, i32, i32, i32)>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UnitStats {
    pub kills: i32,
    pub deaths: i32,
    pub damage: i32,
    pub shots: i32,
    pub plants: i32,
    pub defuses: i32,
}

pub const CHANNEL_NONE: i32 = 0;
pub const CHANNEL_PLANT: i32 = 1;
pub const CHANNEL_DEFUSE: i32 = 2;

pub const CHARGE_CARRIED: i32 = 0;
pub const CHARGE_DROPPED: i32 = 1;
pub const CHARGE_PLANTED: i32 = 2;

#[derive(Clone, Debug)]
pub struct Unit {
    pub id: u32,
    pub team: i32,
    pub kit: i32,
    pub alive: bool,
    pub faulted: bool,
    pub pos: V2,
    pub vel: V2,
    pub facing: i32,
    pub hp: i32,
    pub armor: i32,
    pub credits: i32,
    pub has_charge: bool,
    pub has_kit: bool,
    /// Weapon id per slot (0 primary, 1 sidearm), -1 = empty.
    pub weapons: [i32; 2],
    pub slot: usize,
    pub ammo: [i32; 2],
    pub combat: combat::UnitCombat,
    pub abil: abilities::UnitAbilities,
    pub channel_kind: i32,
    pub channel_left: i32,
    pub blind: i32,
    pub slowed: i32,
    pub revealed: i32,
    pub running: bool,
    pub intent: Intent,
    pub stats: UnitStats,
}

impl Unit {
    pub fn weapon(&self) -> i32 {
        self.weapons[self.slot]
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Projectile {
    pub owner: u32,
    pub team: i32,
    pub weapon: i32,
    /// 0 = bullet; ability id + 1 = a thrown device (`abilities::thrown`).
    pub kind: i32,
    pub pos: V2,
    /// Units per tick.
    pub vel: V2,
    pub travelled: i32,
    pub range: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct Effect {
    pub info: EffectInfo,
    pub owner: u32,
    pub data: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct RoundLog {
    pub round: i32,
    pub winner: i32,
    pub reason: i32,
    pub attackers: i32,
    pub ticks: i32,
}

#[derive(Clone, Debug)]
pub struct World {
    pub rules: Ruleset,
    pub map: Map,
    pub seed: u64,
    pub tick: u32,
    pub round: i32,
    pub phase: i32,
    pub phase_left: i32,
    pub round_start_tick: u32,
    pub score: [i32; 2],
    pub team_size: i32,
    pub units: Vec<Unit>,
    pub projectiles: Vec<Projectile>,
    pub effects: Vec<Effect>,
    pub drops: Vec<DropInfo>,
    pub charge_state: i32,
    pub charge_pos: V2,
    pub team_mem: [Vec<u8>; 2],
    pub draft_mem: [Vec<u8>; 2],
    /// `MEM_BUY` per team, and each unit's credits as the buy phase opened (`T_ROSTER`). See `teamplay`.
    pub buy_mem: [Vec<u8>; 2],
    pub round_credits: Vec<i32>,
    /// Events of the tick that was just resolved, unfogged. Bots read them through `views`.
    pub events: Vec<EventInfo>,
    /// Sounds each team heard during the tick that was just resolved.
    pub team_sounds: [Vec<SoundInfo>; 2],
    /// What each team perceives after the tick that was just resolved (`vision.rs`).
    pub views: [TeamView; 2],
    pub econ: [economy::TeamEcon; 2],
    pub rounds: Vec<RoundLog>,
    pub winner: Option<i32>,
    /// Debug drawings of the current tick: (unit, kind, x0, y0, x1, y1, color). Not part of the state hash.
    pub debug: Vec<crate::DebugDraw>,
}

impl World {
    pub fn new(rules: Ruleset, map: Map, seed: u64) -> World {
        let team_size = map.team_size.min(rules.core.team_size).max(1);
        let mut units = Vec::new();
        for id in 0..(team_size * 2) as u32 {
            units.push(Unit {
                id,
                team: (id as i32) / team_size,
                kit: 0,
                alive: false,
                faulted: false,
                pos: (0, 0),
                vel: (0, 0),
                facing: 0,
                hp: 0,
                armor: 0,
                credits: 0,
                has_charge: false,
                has_kit: false,
                weapons: [-1, -1],
                slot: 1,
                ammo: [0, 0],
                combat: Default::default(),
                abil: Default::default(),
                channel_kind: CHANNEL_NONE,
                channel_left: 0,
                blind: 0,
                slowed: 0,
                revealed: 0,
                running: false,
                intent: Default::default(),
                stats: Default::default(),
            });
        }
        let tm = rules.core.team_mem_bytes.max(0) as usize;
        let dm = rules.core.draft_mem_bytes.max(0) as usize;
        let bm = rules.core.buy_mem_bytes.max(0) as usize;
        let mut w = World {
            rules,
            map,
            seed,
            tick: 0,
            round: 0,
            phase: PHASE_DRAFT,
            phase_left: 1,
            round_start_tick: 0,
            score: [0, 0],
            team_size,
            units,
            projectiles: Vec::new(),
            effects: Vec::new(),
            drops: Vec::new(),
            charge_state: CHARGE_CARRIED,
            charge_pos: (0, 0),
            team_mem: [vec![0; tm], vec![0; tm]],
            draft_mem: [vec![0; dm], vec![0; dm]],
            buy_mem: [vec![0; bm], vec![0; bm]],
            round_credits: Vec::new(),
            events: Vec::new(),
            team_sounds: Default::default(),
            views: Default::default(),
            econ: Default::default(),
            rounds: Vec::new(),
            winner: None,
            debug: Vec::new(),
        };
        economy::on_match_start(&mut w);
        w
    }

    /// Side (SIDE_ATTACK / SIDE_DEFEND) of a team in the current round. Team A attacks the first half.
    pub fn side_of(&self, team: i32) -> i32 {
        let half = self.rules.core.rounds_per_half.max(1);
        let r = (self.round - 1).max(0);
        let a_attacks = if r < 2 * half { r < half } else { (r - 2 * half) % 2 == 0 };
        if (team == TEAM_A) == a_attacks {
            SIDE_ATTACK
        } else {
            SIDE_DEFEND
        }
    }

    pub fn team_with_side(&self, side: i32) -> i32 {
        if self.side_of(TEAM_A) == side {
            TEAM_A
        } else {
            TEAM_B
        }
    }

    pub fn in_action(&self) -> bool {
        self.phase == PHASE_ACTION || self.phase == PHASE_PLANTED
    }

    pub fn max_speed(&self, u: usize) -> i32 {
        let unit = &self.units[u];
        let c = &self.rules.core;
        let mut s = c.run_speed as i64;
        if let Some(w) = self.rules.weapons.get(unit.weapon().max(0) as usize) {
            if unit.weapon() >= 0 {
                s = s * w.move_pct as i64 / 100;
            }
        }
        if let Some(k) = self.rules.kits.get(unit.kit as usize) {
            s = s * k.speed_pct as i64 / 100;
        }
        if unit.slowed > 0 {
            s /= 2;
        }
        s as i32
    }

    pub fn alive_count(&self, team: i32) -> i32 {
        self.units.iter().filter(|u| u.team == team && u.alive).count() as i32
    }

    pub fn emit_event(&mut self, kind: i32, actor: i32, target: i32, pos: V2, value: i32) {
        self.events.push(EventInfo { kind, actor, target, x: pos.0, y: pos.1, value });
    }

    /// Static line of sight plus dynamic blockers (smoke, barriers).
    pub fn sight_clear(&self, a: V2, b: V2) -> bool {
        self.map.segment_wall(a, b).is_none() && !abilities::blocks_vision(self, a, b)
    }

    /// Can unit `u` see point `p` right now (cone + range + line of sight, or the close all-round sense)?
    pub fn can_see_point(&self, u: usize, p: V2) -> bool {
        let unit = &self.units[u];
        if !unit.alive {
            return false;
        }
        let c = &self.rules.core;
        let d2 = fixed::dist2(unit.pos, p);
        let sense = c.sense_radius as i64;
        if d2 <= sense * sense {
            return self.sight_clear(unit.pos, p);
        }
        if unit.blind > 0 {
            return false;
        }
        let range = c.vision_range as i64;
        if d2 > range * range {
            return false;
        }
        let ang = fixed::atan2(p.1 - unit.pos.1, p.0 - unit.pos.0);
        if fixed::angle_diff(unit.facing, ang).abs() > c.fov_half {
            return false;
        }
        self.sight_clear(unit.pos, p)
    }

    // ---------------------------------------------------------------- flow

    fn start_round(&mut self) {
        self.round += 1;
        self.round_start_tick = self.tick;
        self.projectiles.clear();
        self.effects.clear();
        self.drops.clear();
        let survived: Vec<bool> = self.units.iter().map(|u| u.alive).collect();
        let centre = (self.map.width * TILE / 2, self.map.height * TILE / 2);
        let attackers = self.team_with_side(SIDE_ATTACK);
        for i in 0..self.units.len() {
            let team = self.units[i].team;
            let side = self.side_of(team) as usize;
            let slot = (self.units[i].id as i32 - team * self.team_size) as usize;
            let pos = self.map.spawns[side][slot % self.map.spawns[side].len()];
            let hp = self.rules.kits.get(self.units[i].kit as usize).map(|k| k.hp).unwrap_or(self.rules.core.max_hp);
            let u = &mut self.units[i];
            u.alive = true;
            u.pos = pos;
            u.vel = (0, 0);
            u.facing = fixed::atan2(centre.1 - pos.1, centre.0 - pos.0);
            u.hp = hp;
            u.has_charge = team == attackers && slot == 0;
            u.channel_kind = CHANNEL_NONE;
            u.channel_left = 0;
            u.blind = 0;
            u.slowed = 0;
            u.revealed = 0;
            u.running = false;
        }
        self.charge_state = CHARGE_CARRIED;
        self.charge_pos = (0, 0);
        // Team memory does not carry across rounds; draft memory does (read-only).
        for m in self.team_mem.iter_mut() {
            m.fill(0);
        }
        combat::on_round_start(self);
        abilities::on_round_start(self);
        economy::on_round_start(self, &survived);
        crate::teamplay::on_round_start(self);
        self.phase = PHASE_BUY;
        self.phase_left = self.rules.core.buy_ticks;
    }

    pub fn end_round(&mut self, winner: i32, reason: i32) {
        if self.phase == PHASE_ROUND_END || self.phase == PHASE_MATCH_END {
            return;
        }
        self.score[winner as usize] += 1;
        let attackers = self.team_with_side(SIDE_ATTACK);
        self.rounds.push(RoundLog {
            round: self.round,
            winner,
            reason,
            attackers,
            ticks: (self.tick - self.round_start_tick) as i32,
        });
        self.emit_event(EVENT_ROUND_END, winner, -1, self.charge_pos, reason);
        economy::on_round_end(self, winner, reason);
        self.phase = PHASE_ROUND_END;
        self.phase_left = self.rules.round_end_ticks;
    }

    fn after_round(&mut self) {
        let c = self.rules.core;
        let lead = if self.score[0] >= self.score[1] { 0 } else { 1 };
        // CORE CHANGE (unit/economy): `rounds_to_win` only ends the match in regulation. In overtime the
        // economy seam decides, so that both rounds of an overtime pair are played.
        let regulation = self.round <= 2 * c.rounds_per_half.max(1);
        if regulation && self.score[lead] >= c.rounds_to_win && self.score[0] != self.score[1] {
            return self.finish(Some(lead as i32));
        }
        if self.round >= 2 * c.rounds_per_half {
            if let Some(outcome) = economy::after_regulation(self) {
                return self.finish(outcome);
            }
        }
        self.start_round();
    }

    /// End the match. `None` is a draw.
    pub fn finish(&mut self, winner: Option<i32>) {
        self.winner = winner;
        self.phase = PHASE_MATCH_END;
        self.phase_left = 0;
    }

    /// Resolve one tick.
    pub fn step(&mut self) {
        if self.phase == PHASE_MATCH_END {
            return;
        }
        self.events.clear();
        for sounds in self.team_sounds.iter_mut() {
            sounds.clear();
        }
        match self.phase {
            PHASE_DRAFT => {
                self.phase_left -= 1;
                if self.phase_left <= 0 {
                    self.start_round();
                }
            }
            PHASE_BUY => {
                self.phase_left -= 1;
                if self.phase_left <= 0 {
                    self.phase = PHASE_ACTION;
                    self.phase_left = self.rules.core.round_ticks;
                }
            }
            PHASE_ACTION | PHASE_PLANTED => self.step_action(),
            PHASE_ROUND_END => {
                self.phase_left -= 1;
                if self.phase_left <= 0 {
                    self.after_round();
                }
            }
            _ => {}
        }
        for u in self.units.iter_mut() {
            u.intent = Intent::default();
        }
        self.debug.clear();
        self.tick += 1;
        // Running out of ticks is a draw, whatever the score.
        if self.phase != PHASE_MATCH_END && self.tick as i32 >= self.rules.core.max_ticks {
            self.finish(None);
        }
        self.compute_vision();
    }

    fn step_action(&mut self) {
        let n = self.units.len();
        let core = self.rules.core;

        // Where everyone stood, to report the velocity that actually happened (a dash included).
        let start: Vec<V2> = self.units.iter().map(|u| u.pos).collect();

        // 1. intents
        for i in 0..n {
            if !self.units[i].alive {
                continue;
            }
            let intent = self.units[i].intent.clone();
            let mut wants_move = matches!(intent.mv, Some((_, s)) if s > 0);
            if let Some((kind, a, b, c)) = intent.primary {
                match kind {
                    A_FIRE => {
                        if combat::start_fire(self, i, a) == OK {
                            self.cancel_channel(i);
                            if intent.face.is_none() {
                                self.units[i].intent.face = Some(a & 0xFFFF);
                            }
                        }
                    }
                    A_RELOAD => {
                        combat::reload(self, i);
                    }
                    A_SWAP => {
                        combat::swap(self, i, a);
                    }
                    A_PLANT => {
                        if self.units[i].channel_kind == CHANNEL_PLANT || self.can_plant(i) == OK {
                            if self.units[i].channel_kind != CHANNEL_PLANT {
                                combat::cancel(self, i);
                                self.units[i].channel_kind = CHANNEL_PLANT;
                                self.units[i].channel_left = core.plant_ticks;
                                let (p, t) = (self.units[i].pos, self.units[i].team);
                                self.emit_sound(SOUND_PLANT, p, t, core.step_sound_radius);
                            }
                            wants_move = false;
                        }
                    }
                    A_DEFUSE => {
                        if self.units[i].channel_kind == CHANNEL_DEFUSE || self.can_defuse(i) == OK {
                            if self.units[i].channel_kind != CHANNEL_DEFUSE {
                                combat::cancel(self, i);
                                self.units[i].channel_kind = CHANNEL_DEFUSE;
                                self.units[i].channel_left =
                                    if self.units[i].has_kit { core.defuse_kit_ticks } else { core.defuse_ticks };
                                let (p, t) = (self.units[i].pos, self.units[i].team);
                                self.emit_sound(SOUND_DEFUSE, p, t, core.step_sound_radius);
                            }
                            wants_move = false;
                        }
                    }
                    A_ABILITY => {
                        if abilities::use_ability(self, i, a, (b, c)) == OK {
                            combat::on_cast(self, i);
                        }
                    }
                    A_PICKUP => self.pickup(i),
                    A_DROP => self.drop_item(i, a),
                    A_CANCEL => {
                        self.cancel_channel(i);
                        combat::cancel(self, i);
                        abilities::cancel(self, i);
                    }
                    _ => {}
                }
            }
            if wants_move {
                self.cancel_channel(i);
            }
            // facing
            if let Some(target) = self.units[i].intent.face {
                let d = fixed::angle_diff(self.units[i].facing, target).clamp(-core.turn_rate, core.turn_rate);
                self.units[i].facing = (self.units[i].facing + d) & 0xFFFF;
            }
            // velocity
            let max = self.max_speed(i);
            let u = &mut self.units[i];
            match intent.mv {
                Some((ang, speed)) if wants_move => {
                    let s = speed.clamp(0, max);
                    u.vel = fixed::polar(ang, s);
                    u.running = s > core.walk_speed;
                }
                _ => {
                    u.vel = (0, 0);
                    u.running = false;
                }
            }
        }

        // 2. weapons (abilities unit: startups that end happen first)
        abilities::pre_step(self);
        combat::pre_step(self);

        // 3. sub-stepped movement and projectiles
        let sub = core.substeps.max(1);
        for s in 0..sub {
            for i in 0..n {
                if !self.units[i].alive {
                    continue;
                }
                let v = self.units[i].vel;
                let d = (v.0 * (s + 1) / sub - v.0 * s / sub, v.1 * (s + 1) / sub - v.1 * s / sub);
                let p = self.units[i].pos;
                let mut np = p;
                if d.0 != 0 && !self.move_blocked((p.0 + d.0, p.1), core.unit_radius) {
                    np.0 += d.0;
                }
                if d.1 != 0 && !self.move_blocked((np.0, p.1 + d.1), core.unit_radius) {
                    np.1 += d.1;
                }
                self.units[i].pos = np;
            }
            self.step_projectiles(s, sub);
        }
        // Report the velocity that actually happened: a wall can stop all or part of a move.
        for (u, from) in self.units.iter_mut().zip(start) {
            if u.alive {
                u.vel = (u.pos.0 - from.0, u.pos.1 - from.1);
            }
        }
        combat::after_move(self);

        // 4. effects, statuses
        abilities::step(self);
        for u in self.units.iter_mut() {
            u.blind = (u.blind - 1).max(0);
            u.slowed = (u.slowed - 1).max(0);
            u.revealed = (u.revealed - 1).max(0);
        }

        // 5. objective
        self.step_objective();

        // 6. footsteps
        for i in 0..n {
            let u = &self.units[i];
            if u.alive && u.running && u.vel != (0, 0) {
                let (p, t) = (u.pos, u.team);
                self.emit_sound(SOUND_STEP, p, t, core.step_sound_radius);
            }
        }
    }

    fn move_blocked(&self, p: V2, r: i32) -> bool {
        self.map.circle_blocked(p, r) || abilities::blocks_move(self, p, r)
    }

    /// Apply damage to a unit. `attacker` is the unit index credited, if any.
    pub fn damage(&mut self, target: usize, amount: i32, attacker: Option<usize>, weapon: i32) {
        if amount <= 0 || !self.units[target].alive {
            return;
        }
        let pct = self.rules.econ.armor_absorb_pct as i64;
        let t = &mut self.units[target];
        let absorbed = ((amount as i64 * pct / 100) as i32).min(t.armor);
        t.armor -= absorbed;
        let was_alive = t.alive;
        t.hp -= amount - absorbed;
        let pos = t.pos;
        if let Some(a) = attacker {
            self.units[a].stats.damage += amount;
        }
        self.emit_event(EVENT_DAMAGE, attacker.map_or(-1, |a| a as i32), target as i32, pos, amount);
        if was_alive && self.units[target].hp <= 0 {
            self.kill(target, attacker, weapon);
        }
    }

    pub(crate) fn kill(&mut self, target: usize, attacker: Option<usize>, weapon: i32) {
        let pos = self.units[target].pos;
        {
            let t = &mut self.units[target];
            t.alive = false;
            t.hp = 0;
            t.vel = (0, 0);
            t.channel_kind = CHANNEL_NONE;
            t.channel_left = 0;
            t.stats.deaths += 1;
        }
        if self.units[target].has_charge {
            self.units[target].has_charge = false;
            self.charge_state = CHARGE_DROPPED;
            self.charge_pos = pos;
            self.drops.push(DropInfo { kind: DROP_CHARGE, x: pos.0, y: pos.1, item: 0 });
        }
        combat::on_death(self, target);
        if let Some(a) = attacker {
            if self.units[a].team != self.units[target].team {
                self.units[a].stats.kills += 1;
                economy::on_kill(self, a, target, weapon);
            }
        }
        self.emit_event(EVENT_KILL, attacker.map_or(-1, |a| a as i32), target as i32, pos, weapon);
    }

    // ---------------------------------------------------------------- hashing

    /// The state hash: the world hash and both teams' memory hashes, combined.
    pub fn state_hash(&self) -> u64 {
        self.state_hashes().combined()
    }

    pub fn state_hashes(&self) -> crate::StateHashes {
        crate::StateHashes { world: self.world_hash(), memory: [self.memory_hash(0), self.memory_hash(1)] }
    }

    /// Hash of one team's private memory (team, draft and buy scratchpads). Kept apart from `world_hash` so a
    /// replay without that team's memory writes still verifies: memory is read by bots, never by the simulation.
    pub fn memory_hash(&self, team: usize) -> u64 {
        let mut h = crate::Fnv::new();
        h.bytes(&self.team_mem[team]);
        h.bytes(&self.draft_mem[team]);
        h.bytes(&self.buy_mem[team]);
        h.finish()
    }

    /// Hash of every state field that affects simulation, except team memory (`memory_hash`).
    pub fn world_hash(&self) -> u64 {
        let mut h = crate::Fnv::new();
        h.u64(self.seed);
        for v in [self.tick as i32, self.round, self.phase, self.phase_left, self.score[0], self.score[1]] {
            h.i32(v);
        }
        h.i32(self.charge_state);
        h.i32(self.charge_pos.0);
        h.i32(self.charge_pos.1);
        h.i32(self.winner.unwrap_or(-1));
        for u in &self.units {
            for v in [
                u.kit,
                u.alive as i32,
                u.faulted as i32,
                u.pos.0,
                u.pos.1,
                u.vel.0,
                u.vel.1,
                u.facing,
                u.hp,
                u.armor,
                u.credits,
                u.has_charge as i32,
                u.has_kit as i32,
                u.weapons[0],
                u.weapons[1],
                u.slot as i32,
                u.ammo[0],
                u.ammo[1],
                u.channel_kind,
                u.channel_left,
                u.blind,
                u.slowed,
                u.revealed,
                u.stats.kills,
                u.stats.damage,
            ] {
                h.i32(v);
            }
            u.combat.hash(&mut h);
            u.abil.hash(&mut h);
        }
        for p in &self.projectiles {
            for v in [p.owner as i32, p.weapon, p.kind, p.pos.0, p.pos.1, p.vel.0, p.vel.1, p.travelled, p.range] {
                h.i32(v);
            }
        }
        for e in &self.effects {
            h.bytes(bytes_of(&e.info));
            h.i32(e.owner as i32);
            h.i32(e.data);
        }
        for d in &self.drops {
            h.bytes(bytes_of(d));
        }
        for t in 0..2 {
            self.econ[t].hash(&mut h);
            self.views[t].hash(&mut h);
            h.i32(self.team_sounds[t].len() as i32);
            for s in &self.team_sounds[t] {
                h.bytes(bytes_of(s));
            }
        }
        crate::teamplay::hash(self, &mut h);
        h.finish()
    }
}
