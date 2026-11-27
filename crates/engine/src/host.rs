//! What bots see and do: the `ibc_abi::Host` implementation (fogged queries, intent queueing) and the JSON
//! exports. CORE FILE.

use crate::fixed;
use crate::world::*;
use crate::{abilities, combat, economy, teamplay, Input, Match};
use ibc_abi::*;
use serde_json::json;

/// Debug drawings kept per team and tick. Later calls in the tick succeed and are dropped, so one team cannot
/// crowd out the other, and a tick stores at most 4,096 drawings in the replay.
const DEBUG_DRAWS_PER_TEAM: usize = 2048;

fn push<T: Copy>(out: &mut Vec<u8>, v: &T) -> i32 {
    let b = bytes_of(v);
    out.extend_from_slice(b);
    b.len() as i32
}

fn push_row<T: Copy>(out: &mut Vec<u8>, v: Option<T>) -> i32 {
    v.map_or(-ERR_OUT_OF_RANGE, |v| push(out, &v))
}

fn unit_info(w: &World, i: usize, full: bool) -> UnitInfo {
    let u = &w.units[i];
    let mut state = 0;
    let mut flag = |on: bool, bit: i32| {
        if on {
            state |= bit
        }
    };
    flag(u.alive, S_ALIVE);
    flag(u.combat.winding, S_WINDUP);
    flag(u.combat.braced, S_BRACED);
    flag(u.combat.reload > 0, S_RELOADING);
    flag(u.channel_kind == CHANNEL_PLANT, S_PLANTING);
    flag(u.channel_kind == CHANNEL_DEFUSE, S_DEFUSING);
    flag(u.blind > 0, S_BLINDED);
    flag(u.running, S_RUNNING);
    flag(u.slowed > 0, S_SLOWED);
    flag(u.revealed > 0, S_REVEALED);
    flag(u.alive && abilities::casting(w, i), S_CASTING);
    flag(u.has_charge, S_HAS_CHARGE);
    flag(u.has_kit && full, S_HAS_KIT);
    let channel = if u.channel_kind != CHANNEL_NONE { u.channel_left } else { u.combat.channel() };
    UnitInfo {
        id: u.id as i32,
        team: u.team,
        side: w.side_of(u.team),
        kit: u.kit,
        state,
        x: u.pos.0,
        y: u.pos.1,
        facing: u.facing,
        vx: u.vel.0,
        vy: u.vel.1,
        hp: if full { u.hp } else { 0 },
        armor: if full { u.armor } else { 0 },
        weapon: u.weapon(),
        ammo: if full { u.ammo[u.slot] } else { 0 },
        reserve: if full { -1 } else { 0 },
        channel,
        aim: u.combat.aim,
        credits: if full { u.credits } else { 0 },
        charges: if full { u.abil.charges } else { [0, 0] },
        cooldown: if full { u.abil.cooldown } else { [0, 0] },
        recoil: u.combat.recoil,
        blind: if full { u.blind } else { 0 },
        unsteadiness: u.combat.unsteady,
        other_weapon: if full { u.weapons[1 - u.slot.min(1)] } else { 0 },
        fire_cooldown: if full { u.combat.cooldown } else { 0 },
    }
}

/// The units listed in `T_ALLIES` or `T_ENEMIES` for the calling unit, in roster order. Walked, not listed:
/// a team has a handful of units, and `count` and `get` then allocate nothing.
fn unit_rows(w: &World, me: usize, table: u32) -> impl Iterator<Item = usize> + '_ {
    let team = w.units[me].team;
    let seen = w.views[team as usize].units;
    (0..w.units.len()).filter(move |&j| {
        let u = &w.units[j];
        match table {
            T_ALLIES => j != me && u.team == team,
            _ => u.team != team && u.alive && seen & (1 << j) != 0,
        }
    })
}

/// Index of row `ix` of `T_PROJECTILES`, `T_EFFECTS` or `T_DROPS` as the calling unit's team perceives it.
fn perceived_row(w: &World, me: usize, table: u32, ix: usize) -> Option<usize> {
    w.perceived(w.units[me].team, table).get(ix).map(|&j| j as usize)
}

fn match_info(w: &World, me: usize) -> MatchInfo {
    let u = &w.units[me];
    let side = w.side_of(u.team);
    let knows_charge = side == SIDE_ATTACK || w.charge_state == CHARGE_PLANTED;
    let mut charge_pos = w.charge_pos;
    if w.charge_state == CHARGE_CARRIED {
        if let Some(c) = w.units.iter().find(|x| x.has_charge) {
            charge_pos = c.pos;
        }
    }
    MatchInfo {
        tick: w.tick as i32,
        round: w.round,
        phase: w.phase,
        phase_ticks_left: w.phase_left,
        my_team: u.team,
        my_side: side,
        score_mine: w.score[u.team as usize],
        score_theirs: w.score[1 - u.team as usize],
        team_size: w.team_size,
        allies_alive: w.alive_count(u.team),
        enemies_alive: w.alive_count(1 - u.team),
        charge_state: if knows_charge { w.charge_state } else { -1 },
        charge_x: if knows_charge { charge_pos.0 } else { 0 },
        charge_y: if knows_charge { charge_pos.1 } else { 0 },
        fuse_left: if w.phase == PHASE_PLANTED { w.phase_left } else { 0 },
        rounds_to_win: w.rules.core.rounds_to_win,
        seed_lo: w.seed as u32 as i32,
        unit_slot: u.id as i32 - u.team * w.team_size,
    }
}

impl Host for Match {
    fn count(&self, unit: u32, table: u32) -> i32 {
        let w = &self.world;
        let me = unit as usize;
        if me >= w.units.len() {
            return -ERR_BAD_ARG;
        }
        match table {
            T_SELF | T_MATCH | T_MAP | T_RULES_CORE => 1,
            T_ALLIES | T_ENEMIES => unit_rows(w, me, table).count() as i32,
            T_PROJECTILES | T_EFFECTS | T_DROPS => w.perceived(w.units[me].team, table).len() as i32,
            T_SOUNDS => w.team_sounds[w.units[me].team as usize].len() as i32,
            T_MAP_ROW => w.map.height,
            T_WEAPON => w.rules.weapons.len() as i32,
            T_ABILITY => w.rules.abilities.len() as i32,
            T_ITEM => w.rules.items.len() as i32,
            T_KIT => w.rules.kits.len() as i32,
            T_SITES => w.map.sites.len() as i32,
            T_EVENTS => w.views[w.units[me].team as usize].events.len() as i32,
            T_ROSTER => w.units.len() as i32,
            T_ROUNDS => w.rounds.len() as i32,
            _ => -ERR_UNSUPPORTED,
        }
    }

    fn get(&self, unit: u32, table: u32, index: u32, out: &mut Vec<u8>) -> i32 {
        let w = &self.world;
        let me = unit as usize;
        let ix = index as usize;
        if me >= w.units.len() {
            return -ERR_BAD_ARG;
        }
        macro_rules! row {
            ($list:expr) => {
                match $list.get(ix) {
                    Some(v) => push(out, v),
                    None => -ERR_OUT_OF_RANGE,
                }
            };
        }
        match table {
            T_SELF => push(out, &unit_info(w, me, true)),
            T_ALLIES => match unit_rows(w, me, table).nth(ix) {
                Some(j) => push(out, &unit_info(w, j, true)),
                None => -ERR_OUT_OF_RANGE,
            },
            T_ENEMIES => match unit_rows(w, me, table).nth(ix) {
                Some(j) => push(out, &unit_info(w, j, false)),
                None => -ERR_OUT_OF_RANGE,
            },
            T_SOUNDS => row!(w.team_sounds[w.units[me].team as usize]),
            T_PROJECTILES => match perceived_row(w, me, table, ix) {
                Some(j) => {
                    let p = &w.projectiles[j];
                    push(
                        out,
                        &ProjectileInfo {
                            kind: p.kind,
                            team: p.team,
                            x: p.pos.0,
                            y: p.pos.1,
                            vx: p.vel.0,
                            vy: p.vel.1,
                            weapon: p.weapon,
                            owner: p.owner as i32,
                        },
                    )
                }
                None => -ERR_OUT_OF_RANGE,
            },
            T_EFFECTS => match perceived_row(w, me, table, ix) {
                Some(j) => push(out, &w.effects[j].info),
                None => -ERR_OUT_OF_RANGE,
            },
            T_DROPS => match perceived_row(w, me, table, ix) {
                Some(j) => push(out, &w.drops[j]),
                None => -ERR_OUT_OF_RANGE,
            },
            T_MATCH => push(out, &match_info(w, me)),
            T_MAP => {
                let side = w.side_of(w.units[me].team) as usize;
                let centre = |s: usize| {
                    let v = &w.map.spawns[s];
                    let n = v.len().max(1) as i32;
                    (v.iter().map(|p| p.0).sum::<i32>() / n, v.iter().map(|p| p.1).sum::<i32>() / n)
                };
                let (mine, theirs) = (centre(side), centre(1 - side));
                push(
                    out,
                    &MapInfo {
                        width: w.map.width,
                        height: w.map.height,
                        sites: w.map.sites.len() as i32,
                        team_size: w.team_size,
                        spawn_x: mine.0,
                        spawn_y: mine.1,
                        enemy_spawn_x: theirs.0,
                        enemy_spawn_y: theirs.1,
                    },
                )
            }
            T_MAP_ROW => {
                if index >= w.map.height as u32 {
                    return -ERR_OUT_OF_RANGE;
                }
                let a = ix * w.map.width as usize;
                out.extend_from_slice(&w.map.tiles[a..a + w.map.width as usize]);
                w.map.width
            }
            T_RULES_CORE => push(out, &w.rules.core),
            T_WEAPON => row!(w.rules.weapons),
            T_ABILITY => row!(w.rules.abilities),
            T_ITEM => row!(w.rules.items),
            T_KIT => row!(w.rules.kits),
            T_SITES => row!(w.map.sites),
            T_EVENTS => push_row(out, w.event_row(me, ix)),
            T_ROSTER => push_row(out, teamplay::roster_row(w, ix)),
            T_ROUNDS => push_row(out, teamplay::round_row(w, ix)),
            _ => -ERR_UNSUPPORTED,
        }
    }

    /// Recorded only if accepted: a refused call changes nothing, so re-simulation without it reaches the
    /// same state. Every check of every phase must therefore run before anything is written.
    fn act(&mut self, unit: u32, kind: u32, a: i32, b: i32, c: i32) -> i32 {
        let r = self.apply_act(unit, kind, a, b, c);
        if r == OK {
            self.inputs.push(Input::Act { unit, kind, a, b, c });
        }
        r
    }

    fn los(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> i32 {
        self.world.map.segment_wall((x0, y0), (x1, y1)).is_none() as i32
    }

    fn raycast(&self, x: i32, y: i32, angle: i32, max: i32) -> i32 {
        let max = max.clamp(0, 128 * TILE);
        let d = fixed::polar(angle, max);
        match self.world.map.segment_wall((x, y), (x + d.0, y + d.1)) {
            Some(hit) => fixed::dist((x, y), hit).min(max),
            None => max,
        }
    }

    fn mem_read(&self, unit: u32, scope: u32, off: u32, out: &mut [u8]) -> i32 {
        match self.mem_view(unit, scope, off, out.len()) {
            Ok(src) => {
                out.copy_from_slice(src);
                OK
            }
            Err(e) => e,
        }
    }

    /// Accepted writes are recorded per team and scope, not per call (`flush_writes`).
    fn mem_write(&mut self, unit: u32, scope: u32, off: u32, data: &[u8]) -> i32 {
        let r = self.apply_mem_write(unit, scope, off, data);
        if r == OK && !data.is_empty() {
            let team = self.world.units[unit as usize].team as usize;
            let (lo, hi) = (off, off + data.len() as u32);
            let w = self.written[team][scope as usize].get_or_insert(Written { unit, lo, hi });
            *w = Written { unit, lo: w.lo.min(lo), hi: w.hi.max(hi) };
        }
        r
    }

    fn debug_draw(&mut self, unit: u32, kind: u32, x0: i32, y0: i32, x1: i32, y1: i32, color: u32) -> i32 {
        let Some(team) = self.world.units.get(unit as usize).map(|u| u.team as usize) else {
            return ERR_BAD_ARG;
        };
        let kept = &mut self.world.debug[team];
        if kept.len() < DEBUG_DRAWS_PER_TEAM {
            kept.push((unit, kind, x0, y0, x1, y1, color));
        }
        OK
    }
}

/// Most actions `act` accepts from one unit in one tick; later calls get `ERR_NOT_ALLOWED` and change
/// nothing. A bot needs a handful (a move, a facing, a primary action, a purchase per teammate); the cap
/// bounds what the replay stores for one that calls `act` in a loop.
pub const MAX_ACTS: u32 = 64;

/// What one team wrote to one memory scope since the inputs were last taken: every write lies in `lo..hi`,
/// and `unit` made the last one.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Written {
    unit: u32,
    lo: u32,
    hi: u32,
}

fn memory(w: &World, team: usize, scope: u32) -> Option<&[u8]> {
    match scope {
        MEM_TEAM => Some(&w.team_mem[team]),
        MEM_DRAFT => Some(&w.draft_mem[team]),
        MEM_BUY => Some(&w.buy_mem[team]),
        _ => None,
    }
}

impl Match {
    /// `act` without recording it (the replay path). Counts against the unit's `MAX_ACTS` like a live call.
    pub(crate) fn apply_act(&mut self, unit: u32, kind: u32, a: i32, b: i32, c: i32) -> i32 {
        let me = unit as usize;
        let Some(u) = self.world.units.get(me) else { return ERR_BAD_ARG };
        if u.faulted || self.acts[me] >= MAX_ACTS {
            return ERR_NOT_ALLOWED;
        }
        let r = act_now(&mut self.world, me, kind, a, b, c);
        if r == OK {
            self.acts[me] += 1;
        }
        r
    }

    /// Bytes `off..off + len` of the memory `unit`'s team has in `scope`, or the error `mem_read` returns.
    /// `mem_read` is this and a copy; the wasm runner copies the bytes straight into the bot's memory.
    pub fn mem_view(&self, unit: u32, scope: u32, off: u32, len: usize) -> Result<&[u8], i32> {
        let team = self.world.units.get(unit as usize).ok_or(ERR_BAD_ARG)?.team as usize;
        let mem = memory(&self.world, team, scope).ok_or(ERR_BAD_ARG)?;
        let off = off as usize;
        off.checked_add(len).and_then(|end| mem.get(off..end)).ok_or(ERR_OUT_OF_RANGE)
    }

    /// `mem_write` without recording it (the replay path).
    pub(crate) fn apply_mem_write(&mut self, unit: u32, scope: u32, off: u32, data: &[u8]) -> i32 {
        let w = &mut self.world;
        let team = match w.units.get(unit as usize) {
            Some(u) if !u.faulted => u.team as usize,
            _ => return ERR_BAD_ARG,
        };
        let mem = match scope {
            MEM_TEAM if w.in_action() => &mut w.team_mem[team],
            MEM_DRAFT if w.phase == PHASE_DRAFT => &mut w.draft_mem[team],
            MEM_BUY if w.phase == PHASE_BUY => &mut w.buy_mem[team],
            MEM_TEAM | MEM_DRAFT | MEM_BUY => return ERR_NOT_ALLOWED,
            _ => return ERR_BAD_ARG,
        };
        let off = off as usize;
        if off.checked_add(data.len()).map_or(true, |end| end > mem.len()) {
            return ERR_OUT_OF_RANGE;
        }
        mem[off..off + data.len()].copy_from_slice(data);
        OK
    }

    /// Turn what each team wrote since the last flush into inputs: per team and scope, one `MemWrite` of
    /// the bytes as they are now, over the span of every write. Memory never feeds the simulation, so the
    /// end state is all a replay needs, and that is at most a scope's size per team, however many calls.
    pub(crate) fn flush_writes(&mut self) {
        for team in 0..2 {
            for scope in [MEM_TEAM, MEM_DRAFT, MEM_BUY] {
                let Some(s) = self.written[team][scope as usize].take() else { continue };
                let mem = memory(&self.world, team, scope).expect("a written scope exists");
                let data = mem[s.lo as usize..s.hi as usize].to_vec();
                self.inputs.push(Input::MemWrite { unit: s.unit, scope, off: s.lo, data });
            }
        }
    }
}

/// An action of a unit that exists and has not faulted, in the current phase.
fn act_now(w: &mut World, me: usize, kind: u32, a: i32, b: i32, c: i32) -> i32 {
    match w.phase {
        PHASE_DRAFT => match kind {
            A_DRAFT => abilities::draft(w, me, a),
            A_DRAFT_FOR => teamplay::teammate(w, me, b).map_or_else(|e| e, |t| abilities::draft(w, t, a)),
            _ => ERR_NOT_ALLOWED,
        },
        PHASE_BUY => match kind {
            A_BUY => economy::buy(w, me, a),
            A_BUY_FOR => teamplay::teammate(w, me, b).map_or_else(|e| e, |t| economy::buy_for(w, me, t, a)),
            A_FACE => {
                w.units[me].facing = a & 0xFFFF;
                OK
            }
            _ => ERR_NOT_ALLOWED,
        },
        PHASE_ACTION | PHASE_PLANTED => {
            if !w.units[me].alive {
                return ERR_NOT_ALLOWED;
            }
            let check = match kind {
                A_MOVE => {
                    w.units[me].intent.mv = Some((a & 0xFFFF, b.max(0)));
                    return OK;
                }
                A_FACE => {
                    w.units[me].intent.face = Some(a & 0xFFFF);
                    return OK;
                }
                A_FIRE => combat::can_fire(w, me),
                A_PLANT => {
                    if w.units[me].channel_kind == CHANNEL_PLANT {
                        OK
                    } else {
                        w.can_plant(me)
                    }
                }
                A_DEFUSE => {
                    if w.units[me].channel_kind == CHANNEL_DEFUSE {
                        OK
                    } else {
                        w.can_defuse(me)
                    }
                }
                A_ABILITY => abilities::can_use(w, me, a, (b, c)),
                A_SWAP => {
                    if (0..2).contains(&a) && w.units[me].weapons[a as usize] >= 0 {
                        OK
                    } else {
                        ERR_BAD_ARG
                    }
                }
                A_RELOAD | A_PICKUP | A_DROP | A_CANCEL => OK,
                _ => ERR_UNSUPPORTED,
            };
            // Abilities unit: hands are busy during an ability's startup; only A_CANCEL gets through.
            let check =
                if check == OK && kind != A_CANCEL && abilities::casting(w, me) { ERR_NOT_READY } else { check };
            if check == OK {
                w.units[me].intent.primary = Some((kind, a, b, c));
            }
            check
        }
        _ => ERR_NOT_ALLOWED,
    }
}

/// What a viewer needs from the ruleset to draw a match and name what is in it. Frames carry ids and raw
/// state only, so without this a viewer has to keep its own copy of these numbers and names.
pub fn rules_json(r: &crate::Ruleset) -> String {
    let c = &r.core;
    json!({
        "unit_radius": c.unit_radius, "fov_half": c.fov_half, "vision_range": c.vision_range,
        "sense_radius": c.sense_radius, "max_hp": c.max_hp, "plant_ticks": c.plant_ticks,
        "defuse_ticks": c.defuse_ticks, "fuse_ticks": c.fuse_ticks, "weapons": r.weapon_names,
    })
    .to_string()
}

pub fn frame_json(w: &World) -> String {
    let units: Vec<_> = (0..w.units.len())
        .map(|i| {
            let u = &w.units[i];
            let info = unit_info(w, i, true);
            json!({
                "id": u.id, "team": u.team, "side": info.side, "kit": u.kit, "alive": u.alive, "state": info.state,
                "x": u.pos.0, "y": u.pos.1, "facing": u.facing, "vx": u.vel.0, "vy": u.vel.1,
                "hp": u.hp, "armor": u.armor, "weapon": info.weapon, "ammo": info.ammo, "channel": info.channel,
                "aim": info.aim, "credits": u.credits, "seen": w.views[u.team as usize].units.to_string(),
                "kills": u.stats.kills, "deaths": u.stats.deaths,
                "sounds": w.team_sounds[u.team as usize].iter().map(|s| json!([s.kind, s.x, s.y])).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "tick": w.tick, "round": w.round, "phase": w.phase, "phase_left": w.phase_left, "score": w.score,
        "attackers": w.team_with_side(SIDE_ATTACK),
        "charge": { "state": w.charge_state, "x": w.charge_pos.0, "y": w.charge_pos.1 },
        "units": units,
        "projectiles": w.projectiles.iter().map(|p| json!({
            "owner": p.owner, "team": p.team, "weapon": p.weapon, "kind": p.kind,
            "x": p.pos.0, "y": p.pos.1, "vx": p.vel.0, "vy": p.vel.1 })).collect::<Vec<_>>(),
        "effects": w.effects.iter().map(|e| json!({
            "kind": e.info.kind, "team": e.info.team, "x": e.info.x, "y": e.info.y, "radius": e.info.radius,
            "ticks_left": e.info.ticks_left, "x2": e.info.x2, "y2": e.info.y2 })).collect::<Vec<_>>(),
        "drops": w.drops.iter().map(|d| json!({ "kind": d.kind, "x": d.x, "y": d.y, "item": d.item })).collect::<Vec<_>>(),
        "events": w.events.iter().map(|e| json!({
            "kind": e.kind, "actor": e.actor, "target": e.target, "x": e.x, "y": e.y, "value": e.value,
            "dir": e.dir, "src_x": e.src_x, "src_y": e.src_y })).collect::<Vec<_>>(),
        "debug": w.debug.iter().flatten().map(|d| json!([d.0, d.1, d.2, d.3, d.4, d.5, d.6])).collect::<Vec<_>>(),
        "winner": w.winner,
    })
    .to_string()
}

/// `kills` rows are `[tick, round, killer, victim, killer_team, victim_team, weapon]` (`stats::Kill`).
pub fn summary_json(w: &World, stats: &crate::MatchStats) -> String {
    json!({
        "engine": crate::ENGINE_VERSION,
        "ruleset_hash": format!("{:016x}", w.rules.hash()),
        "map": w.map.name, "map_hash": format!("{:016x}", w.map.hash()),
        "seed": w.seed.to_string(),
        "winner": w.winner, "score": w.score, "ticks": w.tick, "team_size": w.team_size,
        "final_hash": format!("{:016x}", w.state_hash()),
        "digest": format!("{:016x}", stats.digest(w)),
        "rounds": w.rounds.iter().enumerate().map(|(i, r)| json!({
            "round": r.round, "winner": r.winner, "reason": r.reason, "attackers": r.attackers, "ticks": r.ticks,
            "planted": stats.planted.get(i).copied().unwrap_or(false),
        })).collect::<Vec<_>>(),
        "kills": stats.kills.iter().map(|k| json!([
            k.tick, k.round, k.killer, k.victim, k.killer_team, k.victim_team, k.weapon,
        ])).collect::<Vec<_>>(),
        "units": w.units.iter().map(|u| json!({
            "id": u.id, "team": u.team, "kit": u.kit, "faulted": u.faulted,
            "kills": u.stats.kills, "deaths": u.stats.deaths, "damage": u.stats.damage, "shots": u.stats.shots,
            "plants": u.stats.plants, "defuses": u.stats.defuses,
        })).collect::<Vec<_>>(),
    })
    .to_string()
}
