//! Utility: a demo of kits and abilities (docs/rules/abilities.md).
//!
//! It drafts a kit by unit slot, so a team fields several, and otherwise plays exactly like the rusher.
//! After the rusher has made its move, at most one ability replaces this tick's primary action:
//! smoke between us and an enemy, flash on an enemy (then look away until it pops), incendiary on an
//! enemy, a barrier when we are hurt, recon and decoy towards the enemy spawn, a dash along our path,
//! and heal on whoever is hurt.
use ibc::*;
use rusher::Rusher;

/// `AbilityRules::effect` of the dash (the ABI has no `EFFECT_*` constant for it yet).
const EFFECT_DASH: i32 = 8;
/// Heal a teammate, or raise a barrier, below this share of 100 hit points.
const HURT_HP: i32 = 60;
/// Do not smoke an enemy closer than this: the cloud would cover us as well.
const SMOKE_MIN_DIST: i32 = 4 * TILE;
/// Recon, decoy and dash wait this many ticks into the round, until the team is on its way.
const OPENING_TICKS: i32 = 6;

pub struct Utility {
    base: Rusher,
    /// Kit the cached ability rules below belong to.
    kit: i32,
    abilities: [Option<AbilityRules>; 2],
    /// Keep facing this angle until this tick: our own flash is about to pop.
    look_away: Option<(i32, i32)>,
}

impl Bot for Utility {
    fn new() -> Self {
        Utility { base: Rusher::new(), kit: -1, abilities: [None, None], look_away: None }
    }

    fn turn(&mut self) {
        let info = match_info();
        if info.phase == PHASE_DRAFT {
            // Kit 0 is the plain one; spread the others over the team.
            let kits = core_rules().kits;
            if kits > 1 {
                let _ = draft(1 + info.unit_slot % (kits - 1));
            }
            return;
        }
        self.base.turn();
        let me = me();
        if !me.alive() {
            return;
        }
        if self.kit != me.kit {
            self.kit = me.kit;
            let slots = kit(me.kit).map_or([-1, -1], |k| k.abilities);
            self.abilities = slots.map(ability);
        }
        match info.phase {
            PHASE_BUY => self.stock_up(&me),
            PHASE_ACTION | PHASE_PLANTED => self.use_abilities(&me, &info),
            _ => {}
        }
    }
}

impl Utility {
    /// Buy charges for both slots while the shop sells them and we can pay.
    fn stock_up(&self, me: &UnitInfo) {
        for it in items().filter(|it| it.kind == ITEM_KIND_ABILITY_CHARGE) {
            let slot = it.value.clamp(0, 1) as usize;
            if let Some(a) = self.abilities[slot] {
                if me.charges[slot] < a.max_charges && me.credits >= it.price + a.price {
                    let _ = buy(it.id);
                }
            }
        }
    }

    fn use_abilities(&mut self, me: &UnitInfo, info: &MatchInfo) {
        if let Some((angle, until)) = self.look_away {
            if info.tick <= until {
                let _ = face(angle);
                return;
            }
            self.look_away = None;
        }
        // `enemies()` is the team's sight; only aim abilities at enemies with no wall in between.
        let enemy = enemies().filter(|e| e.alive() && los(me.pos(), e.pos())).min_by_key(|e| dist2(me.pos(), e.pos()));
        for slot in 0..2 {
            let a = match self.abilities[slot] {
                Some(a) if me.charges[slot] > 0 && me.cooldown[slot] == 0 => a,
                _ => continue,
            };
            let target = match self.target(&a, me, info, enemy.as_ref()) {
                Some(t) => t,
                None => continue,
            };
            if use_ability(slot as i32, target.0, target.1).is_ok() {
                if a.effect == EFFECT_FLASH {
                    let away = angle_to(me.x - target.0, me.y - target.1);
                    let _ = face(away);
                    self.look_away = Some((away, info.tick + tactics::ticks_to_effect(&a, me.pos(), target)));
                }
                return;
            }
        }
    }

    /// Where to use this ability right now, if anywhere.
    fn target(&self, a: &AbilityRules, me: &UnitInfo, info: &MatchInfo, enemy: Option<&UnitInfo>) -> Option<(i32, i32)> {
        let pos = me.pos();
        let map = map_info();
        let opened = core_rules().round_ticks - info.phase_ticks_left >= OPENING_TICKS || info.phase == PHASE_PLANTED;
        let ahead = (map.enemy_spawn_x, map.enemy_spawn_y);
        match a.effect {
            EFFECT_SMOKE => {
                let e = enemy.filter(|e| dist(pos, e.pos()) >= SMOKE_MIN_DIST)?;
                Some(((me.x + e.x) / 2, (me.y + e.y) / 2))
            }
            EFFECT_FLASH | EFFECT_FIRE => enemy.filter(|e| dist(pos, e.pos()) <= a.range).map(|e| e.pos()),
            EFFECT_BARRIER => {
                let e = enemy.filter(|_| me.hp <= HURT_HP)?;
                let towards = angle_to(e.x - me.x, e.y - me.y);
                Some(offset(me.x, me.y, towards, a.range.min(dist(pos, e.pos()) / 2)))
            }
            EFFECT_RECON | EFFECT_DECOY => (enemy.is_none() && opened).then_some(ahead),
            EFFECT_DASH => {
                // Along the way we are already running; never into a fight.
                let speed = len(me.vx, me.vy);
                (enemy.is_none() && opened && speed > 0)
                    .then(|| (me.x + me.vx * a.range / speed, me.y + me.vy * a.range / speed))
                    .filter(|p| p.0 >= 0 && p.1 >= 0 && p.0 <= map.width * TILE && p.1 <= map.height * TILE)
            }
            EFFECT_HEAL => {
                let hurt = |u: &UnitInfo| u.alive() && u.hp <= HURT_HP && dist(pos, u.pos()) <= a.range;
                if hurt(me) {
                    return Some(pos);
                }
                allies().filter(hurt).min_by_key(|u| u.hp).map(|u| u.pos())
            }
            _ => None,
        }
    }
}
