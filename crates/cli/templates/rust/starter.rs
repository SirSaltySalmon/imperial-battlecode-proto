//! {{name}}: a small bot to grow from.
//!
//! Buy:     one thing per buy tick (best affordable weapon, then armour, then a defuse kit).
//! Attack:  everyone runs to the first site; the carrier plants there.
//! Defence: hold the first site; once the charge is planted, run to it and defuse.
//! Always:  shoot the nearest visible enemy (`Gunner` leads the target and waits for a steady aim).
//!
//! Build and play:  ibc build bots/{{name}}  &&  ibc run bots/{{name}}
//! The rules are in docs/RULES.md, the API in docs/SDK-RUST.md and docs/ABI.md.
use ibc::tactics::{shop, Gunner};
use ibc::*;

/// Interact from well inside the allowed reach, so rounding never puts us outside it.
const REACH_PCT: i32 = 70;

pub struct {{type}} {
    core: CoreRules,
    nav: Navigator,
    gun: Gunner,
}

impl Bot for {{type}} {
    fn new() -> Self {
        // Rules and the map never change: read them once. Never hard-code a number the rules give you.
        let core = core_rules();
        {{type}} { core, nav: Navigator::new(), gun: Gunner::new(&core) }
    }

    fn turn(&mut self) {
        let me = me(); // fetch once per tick: every host call costs fuel
        if !me.alive() {
            return;
        }
        let info = match_info();
        match info.phase {
            PHASE_BUY => {
                shop(&me);
            }
            PHASE_ACTION | PHASE_PLANTED => self.fight(&me, &info),
            _ => {}
        }
    }
}

impl {{type}} {
    fn fight(&mut self, me: &UnitInfo, info: &MatchInfo) {
        if self.gun.engage(me).is_some() {
            return; // an enemy is in sight: stand and shoot
        }
        let Some(site) = sites().next() else { return };
        let site = (site.centre_x, site.centre_y);
        let charge = (info.charge_x, info.charge_y);
        let attacking = info.my_side == SIDE_ATTACK;
        if !attacking && info.phase == PHASE_PLANTED {
            if dist(me.pos(), charge) <= self.core.interact_reach * REACH_PCT / 100 {
                let _ = defuse();
                return;
            }
            self.travel(me, charge);
            return;
        }
        let arrived = self.travel(me, site);
        if arrived && attacking && me.has(S_HAS_CHARGE) && info.phase == PHASE_ACTION {
            let _ = plant();
        } else if arrived {
            self.gun.top_up(me);
        }
    }

    /// Walk to `goal` around walls, looking where we go (you only see inside your view cone). True on arrival.
    fn travel(&mut self, me: &UnitInfo, goal: (i32, i32)) -> bool {
        let arrived = self.nav.go(me, goal.0, goal.1, self.core.run_speed).unwrap_or(false);
        if !arrived {
            let _ = face(self.nav.heading);
        }
        arrived
    }
}
