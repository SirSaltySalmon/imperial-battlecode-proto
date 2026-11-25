//! The duels `combat_ttk::ttk` measures: ticks from the first pull until a stationary target dies.
//!
//! The shooter fires when the weapon is ready and the recoil shift (after the wind-up's recovery) is no larger
//! than the angle the target covers. Running: it strafes at the weapon's top speed and aims from where it
//! will be when the bullets leave.

use crate::record::{Catalog, Meta, TtkKey};
use crate::setup::{self, arena};
use crate::take::Take;
use ibc_engine::abi::*;
use ibc_engine::combat::{self, extra_spread};
use ibc_engine::fixed::{self, V2};

const DISTANCES: [i32; 3] = [5, 10, 20];
const CAP: i32 = 100;
const UNITS_PER_RADIAN: i64 = 10430;
const STRAFE_LEG: i32 = 4;

fn strafe_angle(tick: i32) -> i32 {
    if (tick / STRAFE_LEG) % 2 == 0 {
        16384
    } else {
        49152
    }
}

pub fn add(cat: &mut Catalog, heavy: i32) {
    let rules = ibc_engine::Ruleset::builtin();
    for (w, name) in rules.weapons.iter().zip(&rules.weapon_names) {
        for &tiles in &DISTANCES {
            for &running in &[false, true] {
                for &armor in &[0, heavy] {
                    add_one(cat, w.id, name, tiles, running, armor);
                }
            }
        }
    }
}

fn add_one(cat: &mut Catalog, id: i32, name: &str, tiles: i32, running: bool, armor: i32) {
    let mut t = Take::begin(prepare(id, tiles, armor));
    let died = play(&mut t, id, tiles, running);
    let gait = if running { "running" } else { "standing" };
    let plate = if armor == 0 {
        "no armour"
    } else {
        "heavy armour"
    };
    let outcome = match died {
        Some(tick) => tick.to_string(),
        None => "no kill".to_string(),
    };
    t.finish(
        cat,
        Meta {
            id: format!("ttk-{name}-{tiles}-{gait}-{armor}"),
            group: "ttk".to_string(),
            title: format!("{name}, {tiles} tiles, {gait}, {plate}"),
            note: "combat_ttk::ttk. The target stands still. The shooter fires whenever it is ready and \
                   its recoil shift still covers the target, and reloads when the magazine is empty. \
                   Running means it strafes across the line of fire at this weapon's top speed, turning \
                   around every four ticks, and aims from where it will be when the bullets leave."
                .to_string(),
            outcome,
            ttk: Some(TtkKey { weapon: name.to_string(), tiles, running, armor }),
        },
    );
}

fn prepare(id: i32, tiles: i32, armor: i32) -> ibc_engine::Match {
    let mut m = setup::start(&arena(34, 15, 1), 1);
    setup::place(&mut m, 0, setup::at(4, 7), 0);
    setup::place(&mut m, 1, setup::at(4 + tiles, 7), 32768);
    setup::arm(&mut m, 0, id);
    m.world.units[1].armor = armor;
    m
}

/// The step on which the target died, counted the way `combat_ttk` does (`tick + 1`).
fn play(t: &mut Take, id: i32, tiles: i32, running: bool) -> Option<i32> {
    let target: V2 = setup::at(4 + tiles, 7);
    let wr = t.m.world.rules.weapons[id as usize];
    let radius = t.m.world.rules.core.unit_radius as i64;
    let speed = if running { t.m.world.max_speed(0) } else { 0 };
    for tick in 0..CAP {
        let me = t.m.world.units[0].clone();
        if running {
            t.act(0, A_MOVE, strafe_angle(tick), speed);
        }
        let mut from = me.pos;
        for k in 0..wr.windup {
            let step = fixed::polar(strafe_angle(tick + k), speed);
            from = (from.0 + step.0, from.1 + step.1);
        }
        let aim = fixed::atan2(target.1 - from.1, target.0 - from.0);
        t.act(0, A_FACE, aim, 0);
        let covers = radius * UNITS_PER_RADIAN / i64::from(fixed::dist(from, target).max(1));
        let recoil = (me.combat.recoil - wr.recoil_recovery * wr.windup).max(0);
        if me.ammo[me.slot] == 0 {
            t.act(0, A_RELOAD, 0, 0);
        } else if combat::can_fire(&t.m.world, 0) == OK
            && i64::from(extra_spread(&wr, 0, recoil)) / 2 <= covers
        {
            t.act(0, A_FIRE, aim, 0);
        }
        t.step();
        if !t.m.world.units[1].alive {
            return Some(tick + 1);
        }
    }
    None
}
