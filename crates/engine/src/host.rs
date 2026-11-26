//! What bots see and do: the `ibc_abi::Host` implementation (fogged queries, intent queueing) and the JSON
//! exports. CORE FILE.

use crate::fixed;
use crate::world::*;
use crate::{abilities, combat, economy, Input, Match};
use ibc_abi::*;
use serde_json::json;

fn push<T: Copy>(out: &mut Vec<u8>, v: &T) -> i32 {
    let b = bytes_of(v);
    out.extend_from_slice(b);
    b.len() as i32
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
    }
}

/// Indices of the rows of a list table as the calling unit's team perceives it, in a fixed order.
fn rows(w: &World, me: usize, table: u32) -> Vec<usize> {
    let team = w.units[me].team;
    let view = &w.views[team as usize];
    match table {
        T_ALLIES => (0..w.units.len()).filter(|&j| j != me && w.units[j].team == team).collect(),
        T_ENEMIES => (0..w.units.len())
            .filter(|&j| w.units[j].team != team && w.units[j].alive && view.units & (1 << j) != 0)
            .collect(),
        T_PROJECTILES | T_EFFECTS | T_DROPS => w.perceived(team, table),
        _ => Vec::new(),
    }
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
            T_ALLIES | T_ENEMIES | T_PROJECTILES | T_EFFECTS | T_DROPS => rows(w, me, table).len() as i32,
            T_SOUNDS => w.team_sounds[w.units[me].team as usize].len() as i32,
            T_MAP_ROW => w.map.height,
            T_WEAPON => w.rules.weapons.len() as i32,
            T_ABILITY => w.rules.abilities.len() as i32,
            T_ITEM => w.rules.items.len() as i32,
            T_KIT => w.rules.kits.len() as i32,
            T_SITES => w.map.sites.len() as i32,
            T_EVENTS => w.views[w.units[me].team as usize].events.len() as i32,
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
            T_ALLIES => match rows(w, me, table).get(ix) {
                Some(&j) => push(out, &unit_info(w, j, true)),
                None => -ERR_OUT_OF_RANGE,
            },
            T_ENEMIES => match rows(w, me, table).get(ix) {
                Some(&j) => push(out, &unit_info(w, j, false)),
                None => -ERR_OUT_OF_RANGE,
            },
            T_SOUNDS => row!(w.team_sounds[w.units[me].team as usize]),
            T_PROJECTILES => match rows(w, me, table).get(ix) {
                Some(&j) => {
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
            T_EFFECTS => match rows(w, me, table).get(ix) {
                Some(&j) => push(out, &w.effects[j].info),
                None => -ERR_OUT_OF_RANGE,
            },
            T_DROPS => match rows(w, me, table).get(ix) {
                Some(&j) => push(out, &w.drops[j]),
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
                if index as i32 >= w.map.height {
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
            T_EVENTS => row!(w.views[w.units[me].team as usize].events),
            _ => -ERR_UNSUPPORTED,
        }
    }

    fn act(&mut self, unit: u32, kind: u32, a: i32, b: i32, c: i32) -> i32 {
        self.inputs.push(Input::Act { unit, kind, a, b, c });
        let w = &mut self.world;
        let me = unit as usize;
        if me >= w.units.len() {
            return ERR_BAD_ARG;
        }
        if w.units[me].faulted {
            return ERR_NOT_ALLOWED;
        }
        match w.phase {
            PHASE_DRAFT => match kind {
                A_DRAFT => abilities::draft(w, me, a),
                _ => ERR_NOT_ALLOWED,
            },
            PHASE_BUY => match kind {
                A_BUY => economy::buy(w, me, a),
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
                if check == OK {
                    w.units[me].intent.primary = Some((kind, a, b, c));
                }
                check
            }
            _ => ERR_NOT_ALLOWED,
        }
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
        let w = &self.world;
        let team = match w.units.get(unit as usize) {
            Some(u) => u.team as usize,
            None => return ERR_BAD_ARG,
        };
        let mem = match scope {
            MEM_TEAM => &w.team_mem[team],
            MEM_DRAFT => &w.draft_mem[team],
            _ => return ERR_BAD_ARG,
        };
        let off = off as usize;
        if off.checked_add(out.len()).map_or(true, |end| end > mem.len()) {
            return ERR_OUT_OF_RANGE;
        }
        out.copy_from_slice(&mem[off..off + out.len()]);
        OK
    }

    fn mem_write(&mut self, unit: u32, scope: u32, off: u32, data: &[u8]) -> i32 {
        self.inputs.push(Input::MemWrite { unit, scope, off, data: data.to_vec() });
        let w = &mut self.world;
        let team = match w.units.get(unit as usize) {
            Some(u) if !u.faulted => u.team as usize,
            _ => return ERR_BAD_ARG,
        };
        let mem = match scope {
            MEM_TEAM => &mut w.team_mem[team],
            MEM_DRAFT if w.phase == PHASE_DRAFT => &mut w.draft_mem[team],
            MEM_DRAFT => return ERR_NOT_ALLOWED,
            _ => return ERR_BAD_ARG,
        };
        let off = off as usize;
        if off.checked_add(data.len()).map_or(true, |end| end > mem.len()) {
            return ERR_OUT_OF_RANGE;
        }
        mem[off..off + data.len()].copy_from_slice(data);
        OK
    }

    fn debug_draw(&mut self, unit: u32, kind: u32, x0: i32, y0: i32, x1: i32, y1: i32, color: u32) -> i32 {
        if self.world.debug.len() < 4096 {
            self.world.debug.push((unit, kind, x0, y0, x1, y1, color));
        }
        OK
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
            "kind": e.kind, "actor": e.actor, "target": e.target, "x": e.x, "y": e.y, "value": e.value })).collect::<Vec<_>>(),
        "debug": w.debug.iter().map(|d| json!([d.0, d.1, d.2, d.3, d.4, d.5, d.6])).collect::<Vec<_>>(),
        "winner": w.winner,
    })
    .to_string()
}

pub fn summary_json(w: &World) -> String {
    json!({
        "engine": crate::ENGINE_VERSION,
        "ruleset_hash": format!("{:016x}", w.rules.hash()),
        "map": w.map.name, "map_hash": format!("{:016x}", w.map.hash()),
        "seed": w.seed.to_string(),
        "winner": w.winner, "score": w.score, "ticks": w.tick, "team_size": w.team_size,
        "final_hash": format!("{:016x}", w.state_hash()),
        "rounds": w.rounds.iter().map(|r| json!({
            "round": r.round, "winner": r.winner, "reason": r.reason, "attackers": r.attackers, "ticks": r.ticks,
        })).collect::<Vec<_>>(),
        "units": w.units.iter().map(|u| json!({
            "id": u.id, "team": u.team, "kit": u.kit, "faulted": u.faulted,
            "kills": u.stats.kills, "deaths": u.stats.deaths, "damage": u.stats.damage, "shots": u.stats.shots,
            "plants": u.stats.plants, "defuses": u.stats.defuses,
        })).collect::<Vec<_>>(),
    })
    .to_string()
}
