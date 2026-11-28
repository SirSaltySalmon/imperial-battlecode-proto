//! The objective and items of `World`: plant and defuse channels, the fuse and the detonation blast, round
//! end conditions, picking up and dropping the charge and weapons. Core code, split from `world.rs` to keep it
//! readable.

use crate::world::*;
use crate::{combat, economy, fixed};
use ibc_abi::*;

impl World {
    pub fn cancel_channel(&mut self, u: usize) {
        self.units[u].channel_kind = CHANNEL_NONE;
        self.units[u].channel_left = 0;
    }

    pub fn can_plant(&self, u: usize) -> i32 {
        let unit = &self.units[u];
        if self.phase != PHASE_ACTION || !unit.alive || !unit.has_charge {
            return ERR_NOT_ALLOWED;
        }
        if self.map.site_at(unit.pos).is_none() {
            return ERR_OUT_OF_RANGE;
        }
        OK
    }

    pub fn can_defuse(&self, u: usize) -> i32 {
        let unit = &self.units[u];
        if self.phase != PHASE_PLANTED || !unit.alive || self.side_of(unit.team) != SIDE_DEFEND {
            return ERR_NOT_ALLOWED;
        }
        let reach = self.rules.core.interact_reach as i64;
        if fixed::dist2(unit.pos, self.charge_pos) > reach * reach {
            return ERR_OUT_OF_RANGE;
        }
        OK
    }

    pub(crate) fn pickup(&mut self, u: usize) {
        let reach = self.rules.core.pickup_reach as i64;
        let pos = self.units[u].pos;
        let side = self.side_of(self.units[u].team);
        let mut best: Option<(usize, i64)> = None;
        for (i, d) in self.drops.iter().enumerate() {
            if d.kind == DROP_CHARGE && side != SIDE_ATTACK {
                continue;
            }
            let d2 = fixed::dist2(pos, (d.x, d.y));
            // The charge always wins over a weapon in reach, so a dropped gun can never block the objective.
            let key = if d.kind == DROP_CHARGE { -1 } else { d2 };
            if d2 <= reach * reach && best.map_or(true, |(_, b)| key < b) {
                best = Some((i, key));
            }
        }
        if let Some((i, _)) = best {
            let d = self.drops.remove(i);
            if d.kind == DROP_CHARGE {
                self.units[u].has_charge = true;
                self.charge_state = CHARGE_CARRIED;
            } else {
                combat::on_pickup(self, u, d);
            }
        }
    }

    pub(crate) fn drop_item(&mut self, u: usize, kind: i32) {
        let pos = self.units[u].pos;
        if kind == DROP_CHARGE && self.units[u].has_charge {
            self.units[u].has_charge = false;
            self.charge_state = CHARGE_DROPPED;
            self.charge_pos = pos;
            self.drops.push(DropInfo {
                kind: DROP_CHARGE,
                x: pos.0,
                y: pos.1,
                item: 0,
                ammo: 0,
            });
        } else if kind == DROP_WEAPON {
            combat::on_drop(self, u);
        }
    }

    pub(crate) fn step_objective(&mut self) {
        let n = self.units.len();
        let attackers = self.team_with_side(SIDE_ATTACK);
        let defenders = 1 - attackers;
        for i in 0..n {
            if !self.units[i].alive || self.units[i].channel_kind == CHANNEL_NONE {
                continue;
            }
            match self.units[i].channel_kind {
                CHANNEL_PLANT => {
                    if self.can_plant(i) != OK {
                        self.cancel_channel(i);
                        continue;
                    }
                    self.units[i].channel_left -= 1;
                    if self.units[i].channel_left <= 0 {
                        let pos = self.units[i].pos;
                        self.cancel_channel(i);
                        self.units[i].has_charge = false;
                        self.units[i].stats.plants += 1;
                        self.charge_state = CHARGE_PLANTED;
                        self.charge_pos = pos;
                        self.phase = PHASE_PLANTED;
                        self.phase_left = self.rules.core.fuse_ticks + 1; // +1: decremented below this tick
                        self.emit_event(
                            EVENT_PLANT,
                            i as i32,
                            -1,
                            pos,
                            self.map.site_at(pos).unwrap_or(0) as i32,
                        );
                        economy::on_plant(self, i);
                    }
                }
                CHANNEL_DEFUSE => {
                    if self.can_defuse(i) != OK {
                        self.cancel_channel(i);
                        continue;
                    }
                    self.units[i].channel_left -= 1;
                    if self.units[i].channel_left <= 0 {
                        self.cancel_channel(i);
                        self.units[i].stats.defuses += 1;
                        self.emit_event(EVENT_DEFUSE, i as i32, -1, self.charge_pos, 0);
                        return self.end_round(defenders, ROUND_WIN_DEFUSE);
                    }
                }
                _ => {}
            }
        }
        self.phase_left -= 1;
        let (att_alive, def_alive) = (self.alive_count(attackers), self.alive_count(defenders));
        if self.phase == PHASE_PLANTED {
            if self.phase_left <= 0 {
                self.emit_event(EVENT_DETONATE, -1, -1, self.charge_pos, 0);
                self.blast();
                return self.end_round(attackers, ROUND_WIN_DETONATION);
            }
            if def_alive == 0 {
                return self.end_round(attackers, ROUND_WIN_ELIMINATION);
            }
        } else {
            if att_alive == 0 {
                return self.end_round(defenders, ROUND_WIN_ELIMINATION);
            }
            if def_alive == 0 {
                return self.end_round(attackers, ROUND_WIN_ELIMINATION);
            }
            if self.phase_left <= 0 {
                return self.end_round(defenders, ROUND_WIN_TIMEOUT);
            }
        }
    }

    /// Detonation: every living unit within `blast_radius` of the charge dies, both teams, through walls. The
    /// kill has no attacker (`EVENT_KILL` with actor -1, weapon -1): no reward and no kill for the tie-break.
    pub(crate) fn blast(&mut self) {
        let r = self.rules.core.blast_radius.max(0) as i64;
        for i in 0..self.units.len() {
            if self.units[i].alive && fixed::dist2(self.units[i].pos, self.charge_pos) <= r * r {
                self.kill(i, None, -1);
            }
        }
    }
}
