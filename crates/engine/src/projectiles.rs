//! The projectile step of `World::step_action` (step 3, once per substep): bullets and thrown ability devices
//! move, swept against walls, effects that stop them and (bullets only) unit circles; hits of one substep are
//! applied together so simultaneous shots trade. Core code, split from `world.rs` to keep it readable.

use crate::fixed;
use crate::fixed::V2;
use crate::vision::HitFrom;
use crate::world::{Projectile, Trail, World};
use crate::{abilities, combat};

impl World {
    pub(crate) fn step_projectiles(&mut self, s: i32, sub: i32) {
        let radius = self.rules.core.unit_radius;
        let mut hits: Vec<(usize, Projectile)> = Vec::new();
        // Abilities unit: projectiles stopped by a dynamic effect, with the point they were heading for.
        let mut blocked: Vec<(Projectile, V2)> = Vec::new();
        let mut landed: Vec<(Projectile, V2)> = Vec::new();
        let mut keep = Vec::with_capacity(self.projectiles.len());
        let projectiles = std::mem::take(&mut self.projectiles);
        for mut p in projectiles {
            let d = (
                p.vel.0 * (s + 1) / sub - p.vel.0 * s / sub,
                p.vel.1 * (s + 1) / sub - p.vel.1 * s / sub,
            );
            let a = p.pos;
            let b = (a.0 + d.0, a.1 + d.1);
            let seg_len = fixed::dist(a, b).max(1);
            // Earliest stop along the segment, as a Q14 fraction.
            let mut stop: Option<i64> = None;
            if let Some(hit) = self.map.segment_wall(a, b) {
                stop = Some(fixed::dist(a, hit) as i64 * fixed::ONE / seg_len as i64);
            }
            let mut by_effect = false;
            if let Some(t) = abilities::blocks_segment(self, a, b) {
                by_effect = stop.map_or(true, |s| t < s);
                stop = Some(stop.map_or(t, |s| s.min(t)));
            }
            if p.kind != 0 {
                // Abilities unit: a thrown device flies over units and lands at a wall, a barrier or its range.
                match abilities::fly(self, &mut p, a, b, stop, (s, sub)) {
                    Some(at) => landed.push((p, at)),
                    None => keep.push(p),
                }
                continue;
            }
            // For the trail of a bullet that ends here: where it was when the tick began (the per-substep steps
            // above sum to exactly `vel * s / sub`), and the point a fraction `t` (Q14) along this substep.
            let start = (a.0 - p.vel.0 * s / sub, a.1 - p.vel.1 * s / sub);
            let at = |t: i64| {
                (
                    a.0 + ((b.0 - a.0) as i64 * t / fixed::ONE) as i32,
                    a.1 + ((b.1 - a.1) as i64 * t / fixed::ONE) as i32,
                )
            };
            let (owner, team, weapon, vel) = (p.owner, p.team, p.weapon, p.vel);
            let trail = move |to: V2| Trail {
                owner,
                team,
                weapon,
                from: start,
                to,
                vel,
            };
            let mut victim: Option<(usize, i64)> = None;
            for (j, u) in self.units.iter().enumerate() {
                if !u.alive || u.team == p.team || u.id == p.owner {
                    continue;
                }
                if let Some(t) = fixed::segment_circle(a, b, u.pos, radius) {
                    if victim.map_or(true, |(_, bt)| t < bt) {
                        victim = Some((j, t));
                    }
                }
            }
            if let Some((j, t)) = victim {
                if stop.map_or(true, |st| t <= st) {
                    p.travelled += (seg_len as i64 * t / fixed::ONE) as i32;
                    self.trails.push(trail(at(t)));
                    hits.push((j, p));
                    continue;
                }
            }
            if let Some(st) = stop {
                self.trails.push(trail(at(st)));
                if by_effect {
                    blocked.push((p, b));
                }
                continue;
            }
            p.pos = b;
            p.travelled += seg_len;
            if p.travelled < p.range {
                keep.push(p);
            } else {
                self.trails.push(trail(b));
            }
        }
        self.projectiles = keep;
        // Apply all hits of this substep together so simultaneous shots trade. The victim learns the bullet's
        // heading, not where its shooter stands.
        for (j, p) in hits {
            let dmg = combat::bullet_damage(self, &p);
            self.damage_from(j, dmg, Some(p.owner as usize), p.weapon, HitFrom::Bullet(p.vel));
        }
        // Abilities unit: tell the effect what it stopped (barriers have hit points).
        if !blocked.is_empty() {
            abilities::on_blocked(self, &blocked);
        }
        if !landed.is_empty() {
            abilities::on_landed(self, &landed);
        }
    }
}
