//! The duels `combat_ttk::ttk` measures: ticks from the first pull until a stationary target dies.
//!
//! The shooter stands and fires when the weapon is ready and the pellets nearest the aim would still land.
//! Running: it strafes at the weapon's top speed for `RUN_IN` ticks just before the first pull.

use crate::record::{Catalog, Meta, TtkKey};
use crate::setup::{self, arena};
use crate::take::Take;
use ibc_engine::abi::*;
use ibc_engine::combat::{self, startup_ticks};
use ibc_engine::fixed::{self, V2};

/// The distances of `combat_ttk::DISTANCES`; a test checks the two lists agree.
pub const DISTANCES: [i32; 5] = [2, 5, 10, 15, 20];
const CAP: i32 = 100;
const UNITS_PER_RADIAN: i64 = 10430;
/// Ticks of strafing before the first pull of a running duel (they are recorded before the clock starts).
pub(crate) const RUN_IN: i32 = 8;

fn strafe_angle(tick: i32) -> i32 {
    if tick < RUN_IN / 2 {
        16384
    } else {
        49152
    }
}

/// The armour on the target in each duel: none, then the lightest and the heaviest the shop sells, as
/// `combat_ttk` tabulates them (a value the shop does not sell twice is played once).
pub fn armors(rules: &ibc_engine::Ruleset) -> Vec<i32> {
    let values = || {
        rules
            .items
            .iter()
            .filter(|item| item.kind == ITEM_KIND_ARMOR)
            .map(|item| item.value)
    };
    let mut out = vec![0];
    for v in [values().min(), values().max()].into_iter().flatten() {
        if !out.contains(&v) {
            out.push(v);
        }
    }
    out
}

pub fn add(cat: &mut Catalog, armors: &[i32]) {
    let rules = ibc_engine::Ruleset::builtin();
    let heavy = armors.iter().copied().max().unwrap_or(0);
    for (w, name) in rules.weapons.iter().zip(&rules.weapon_names) {
        for &tiles in &DISTANCES {
            for &running in &[false, true] {
                for &armor in armors {
                    add_one(cat, w.id, name, tiles, running, armor, heavy);
                }
            }
        }
    }
}

fn add_one(
    cat: &mut Catalog,
    id: i32,
    name: &str,
    tiles: i32,
    running: bool,
    armor: i32,
    heavy: i32,
) {
    let mut t = Take::begin(prepare(id, tiles, armor));
    let died = play(&mut t, id, tiles, running);
    let gait = if running { "after running" } else { "standing" };
    let plate = match armor {
        0 => "no armour".to_string(),
        a if a == heavy => format!("heavy armour ({a})"),
        a => format!("light armour ({a})"),
    };
    let outcome = match died {
        Some(tick) => tick.to_string(),
        None => "no kill".to_string(),
    };
    t.finish(
        cat,
        Meta {
            id: format!("ttk-{name}-{tiles}-{}-{armor}", if running { "running" } else { "standing" }),
            group: "ttk".to_string(),
            title: format!("{name}, {tiles} tiles, {gait}, {plate}"),
            note: "combat_ttk::ttk. The target stands still. The shooter stands and fires whenever it is \
                   ready and the pellets nearest the aim still cover the target, and reloads when the \
                   magazine is empty. After running means it strafed across the line of fire at this \
                   weapon's top speed for eight ticks just before the first pull, so it starts up unsteady."
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
    let radius = i64::from(t.m.world.rules.core.unit_radius);
    let fov = t.m.world.rules.core.fov_half;
    if running {
        let speed = t.m.world.max_speed(0);
        for tick in 0..RUN_IN {
            t.act(0, A_MOVE, strafe_angle(tick), speed);
            t.step();
        }
    }
    for tick in 0..CAP {
        let me = t.m.world.units[0].clone();
        let aim = fixed::atan2(target.1 - me.pos.1, target.0 - me.pos.0);
        t.act(0, A_FACE, aim, 0);
        let covers = radius * UNITS_PER_RADIAN / i64::from(fixed::dist(me.pos, target).max(1));
        let c = me.combat;
        let wait = if c.braced {
            0
        } else {
            startup_ticks(&wr, c.unsteady)
        };
        let heat = (c.recoil - wr.recoil_recovery * wait).max(0);
        let inner = if wr.pellets % 2 == 0 {
            combat::fan(&wr, 0, 0, fov, heat).step / 2
        } else {
            0
        };
        if me.ammo[me.slot] == 0 {
            t.act(0, A_RELOAD, 0, 0);
        } else if combat::can_fire(&t.m.world, 0) == OK && i64::from(inner) <= covers {
            t.act(0, A_FIRE, aim, 0);
        }
        t.step();
        if !t.m.world.units[1].alive {
            return Some(tick + 1);
        }
    }
    None
}
