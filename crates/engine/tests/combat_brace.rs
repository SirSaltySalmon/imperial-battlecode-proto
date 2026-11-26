//! Unsteadiness and the brace: moving lengthens the next startup and standing still settles it, a move
//! cancels a startup, and a unit that stands and keeps pulling fires at the weapon's cadence until a move,
//! a skipped pull, a reload or a swap breaks the brace. Also the velocity bots see: what actually happened.

mod combat_common;

use combat_common::*;
use ibc_engine::abi::*;
use ibc_engine::combat::{self, startup_ticks, unsteadiness_after};
use ibc_engine::{Match, Ruleset};

const ME: usize = 0;
const FOE: usize = 1;

/// Shooter in the middle of the left side looking along +x, the other unit out of the way behind it.
fn range(weapon: i32) -> Match {
    let mut m = start(&arena(34, 15, 1), 1);
    place(&mut m, ME, at(4, 7), 0);
    place(&mut m, FOE, at(1, 1), 0);
    arm(&mut m, ME, weapon);
    m
}

fn id(name: &str) -> i32 {
    weapon_id(&Ruleset::builtin(), name)
}

/// Move up and down (legs of three ticks, so the unit stays in the arena) at `speed` for `ticks` ticks.
fn pace(m: &mut Match, speed: i32, ticks: i32) {
    for t in 0..ticks {
        let angle = if (t / 3) % 2 == 0 { 16384 } else { 49152 };
        assert_eq!(m.act(ME as u32, A_MOVE, angle, speed, 0), OK);
        m.step();
    }
}

/// `pace` at the weapon's top speed.
fn run(m: &mut Match, ticks: i32) {
    let speed = m.world.max_speed(ME);
    pace(m, speed, ticks);
}

fn stand(m: &mut Match, ticks: i32) {
    for _ in 0..ticks {
        m.step();
    }
}

/// Pull now (aiming along +x) and hold the trigger; ticks from the pull to the bullets leaving.
fn startup(m: &mut Match) -> i32 {
    let n = shots(m, ME);
    for t in 0..40 {
        m.act(ME as u32, A_FIRE, 0, 0, 0);
        m.step();
        if shots(m, ME) > n {
            return t;
        }
    }
    panic!("no shot");
}

/// After a first shot, keep pulling for `ticks` ticks; the ticks (counted from now) on which bullets left.
fn hold(m: &mut Match, ticks: i32) -> Vec<i32> {
    let mut out = Vec::new();
    for t in 0..ticks {
        let n = shots(m, ME);
        m.act(ME as u32, A_FIRE, 0, 0, 0);
        m.step();
        if shots(m, ME) > n {
            out.push(t);
        }
    }
    out
}

fn me(m: &Match) -> UnitInfo {
    let mut buf = Vec::new();
    assert!(m.get(ME as u32, T_SELF, 0, &mut buf) > 0);
    from_bytes(&buf).unwrap()
}

#[test]
fn running_then_firing_starts_up_slower_than_walking_than_standing() {
    let r = Ruleset::builtin();
    let walk = r.core.walk_speed;
    for w in &r.weapons {
        let measure = |speed: i32| {
            let mut m = range(w.id);
            let speed = if speed < 0 {
                m.world.max_speed(ME)
            } else {
                speed
            };
            pace(&mut m, speed, 3);
            let heat = m.world.units[ME].combat.unsteady;
            let s = startup(&mut m);
            assert_eq!(
                s,
                startup_ticks(w, heat),
                "weapon {}: the startup the formula gives",
                w.id
            );
            s
        };
        let (still, walked, ran) = (measure(0), measure(walk), measure(-1));
        assert_eq!(
            still, w.windup,
            "weapon {}: a steady unit starts up in `windup`",
            w.id
        );
        assert!(
            still <= walked && walked <= ran,
            "weapon {}: {still} {walked} {ran}",
            w.id
        );
    }
    let rifle = r.weapons[id("rifle") as usize];
    let mut m = range(rifle.id);
    let s = startup(&mut m);
    let mut m = range(rifle.id);
    pace(&mut m, walk, 3);
    let wk = startup(&mut m);
    let mut m = range(rifle.id);
    run(&mut m, 3);
    let rn = startup(&mut m);
    assert!(
        s < wk && wk < rn,
        "rifle: standing {s}, walking {wk}, running {rn}"
    );
}

#[test]
fn one_tick_of_standing_does_not_settle_a_run() {
    for name in ["rifle", "marksman", "lmg", "shotgun"] {
        let mut m = range(id(name));
        let wr = weapon(&m, id(name));
        run(&mut m, 8);
        assert_eq!(
            m.world.units[ME].combat.unsteady, wr.unsteady_max,
            "{name}: a run reaches the cap"
        );
        stand(&mut m, 1);
        let s = startup(&mut m);
        assert!(
            s > wr.windup + 1,
            "{name}: startup {s} after one tick of standing"
        );

        // Standing long enough settles it completely.
        let mut m = range(id(name));
        run(&mut m, 8);
        stand(&mut m, wr.unsteady_max / wr.unsteady_decay + 1);
        assert_eq!(m.world.units[ME].combat.unsteady, 0);
        assert_eq!(startup(&mut m), wr.windup, "{name}");
    }
}

#[test]
fn unsteadiness_follows_the_formula_and_bots_see_it() {
    let rifle = Ruleset::builtin().weapons[id("rifle") as usize];
    let mut m = range(rifle.id);
    let mut model = 0;
    let speed = m.world.max_speed(ME);
    for t in 0..12 {
        let moving = t < 6;
        if moving {
            m.act(
                ME as u32,
                A_MOVE,
                if t < 3 { 16384 } else { 49152 },
                speed,
                0,
            );
        }
        m.step();
        model = unsteadiness_after(&rifle, model, if moving { speed } else { 0 });
        assert_eq!(m.world.units[ME].combat.unsteady, model, "tick {t}");
        assert_eq!(me(&m).unsteadiness, model);
    }
    // A walk that adds no more than the decay holds the heat at what one tick adds.
    assert!(unsteadiness_after(&rifle, 0, 350) <= rifle.unsteady_decay);
    assert_eq!(
        unsteadiness_after(&rifle, unsteadiness_after(&rifle, 0, 350), 350),
        unsteadiness_after(&rifle, 0, 350)
    );
    // The cap limits only what moving adds: heat from a weapon with a higher cap just decays.
    let pistol = Ruleset::builtin().weapons[id("pistol") as usize];
    assert!(pistol.unsteady_max < 16);
    assert_eq!(
        unsteadiness_after(&pistol, 16, 0),
        16 - pistol.unsteady_decay
    );
    assert_eq!(
        unsteadiness_after(&pistol, 16, 700),
        16 - pistol.unsteady_decay
    );
    // It is part of the state hash.
    let mut a = range(rifle.id);
    let b = range(rifle.id);
    a.world.units[ME].combat.unsteady = 3;
    assert_ne!(a.world.state_hash(), b.world.state_hash());
}

#[test]
fn smg_runs_and_guns() {
    let r = Ruleset::builtin();
    let smg = r.weapons[id("smg") as usize];
    let mut m = range(smg.id);
    run(&mut m, 8);
    assert_eq!(
        startup(&mut m),
        smg.windup,
        "running costs the smg no startup"
    );
    for name in ["rifle", "marksman", "lmg"] {
        let mut m = range(id(name));
        run(&mut m, 8);
        assert!(startup(&mut m) > weapon(&m, id(name)).windup, "{name}");
    }
}

#[test]
fn a_move_cancels_a_startup_and_a_pull_on_a_moving_tick_is_refused() {
    let id = id("marksman");
    let wr = Ruleset::builtin().weapons[id as usize];
    // Move during the startup: no shot, nothing spent, nobody slides to a halt.
    let mut m = range(id);
    m.act(ME as u32, A_FIRE, 0, 0, 0);
    m.step();
    assert!(m.world.units[ME].combat.winding);
    m.act(ME as u32, A_MOVE, 16384, 300, 0);
    m.step();
    assert!(!m.world.units[ME].combat.winding);
    assert_eq!(m.world.units[ME].vel, (0, 300), "the move happens in full");
    stand(&mut m, wr.windup + 2);
    assert_eq!((shots(&m, ME), m.world.units[ME].ammo[0]), (0, wr.magazine));

    // Move queued first: the pull is refused at once. Pull first, then move: refused when the tick resolves.
    let mut m = range(id);
    m.act(ME as u32, A_MOVE, 16384, 300, 0);
    assert_eq!(m.act(ME as u32, A_FIRE, 0, 0, 0), ERR_NOT_ALLOWED);
    m.step();
    assert!(!m.world.units[ME].combat.winding);
    assert_eq!(m.act(ME as u32, A_FIRE, 0, 0, 0), OK);
    m.act(ME as u32, A_MOVE, 16384, 300, 0);
    m.step();
    assert!(!m.world.units[ME].combat.winding);
    stand(&mut m, wr.windup + 2);
    assert_eq!(shots(&m, ME), 0);
    // A zero-speed move is no move.
    m.act(ME as u32, A_MOVE, 16384, 0, 0);
    assert_eq!(m.act(ME as u32, A_FIRE, 0, 0, 0), OK);
}

#[test]
fn braced_fire_follows_the_cooldown_and_shows_on_the_unit() {
    for wid in 0..Ruleset::builtin().weapons.len() as i32 {
        let mut m = range(wid);
        let wr = weapon(&m, wid);
        assert_eq!(startup(&mut m), wr.windup);
        assert!(m.world.units[ME].combat.braced);
        assert!(me(&m).state & S_BRACED != 0 && me(&m).state & S_WINDUP == 0);
        let pulls = (wr.magazine - 1).min(3);
        let ticks = pulls * (1 + wr.cooldown);
        let expect: Vec<i32> = (1..=pulls).map(|k| k * (1 + wr.cooldown) - 1).collect();
        assert_eq!(
            hold(&mut m, ticks),
            expect,
            "weapon {wid}: one shot every 1 + cooldown ticks"
        );
    }
}

/// Fire once to brace, wait out the cooldown, do `between` on the tick the weapon is ready again (and stand
/// still for the rest of that tick), then pull and return the startup of that pull.
fn after_brace(name: &str, between: impl Fn(&mut Match)) -> i32 {
    let mut m = range(id(name));
    let wr = weapon(&m, id(name));
    startup(&mut m);
    // Cooldown ticks: pulls are refused, which does not break the brace.
    for _ in 0..wr.cooldown {
        assert_eq!(m.act(ME as u32, A_FIRE, 0, 0, 0), ERR_NOT_READY);
        m.step();
        assert!(m.world.units[ME].combat.braced);
    }
    between(&mut m);
    startup(&mut m)
}

#[test]
fn moving_skipping_a_pull_reloading_or_swapping_breaks_the_brace_and_turning_does_not() {
    let wr = Ruleset::builtin().weapons[id("rifle") as usize];
    let braced = 0;
    let fresh = wr.windup;
    assert_eq!(after_brace("rifle", |_| {}), braced, "still braced");
    assert_eq!(
        after_brace("rifle", |m| {
            m.act(ME as u32, A_FACE, 20000, 0, 0);
            assert_eq!(m.act(ME as u32, A_FIRE, 20000, 0, 0), OK);
            m.step();
            for _ in 0..wr.cooldown {
                m.act(ME as u32, A_FACE, 40000, 0, 0);
                m.step();
            }
        }),
        braced,
        "turning while firing keeps the brace"
    );
    let moved = after_brace("rifle", |m| {
        m.act(ME as u32, A_MOVE, 16384, 100, 0);
        m.step();
    });
    assert!(moved >= fresh, "a move breaks it: startup {moved}");
    assert_eq!(
        after_brace("rifle", |m| m.step()),
        fresh,
        "a ready tick without a pull lets go"
    );
    let reloaded = after_brace("rifle", |m| {
        assert_eq!(m.act(ME as u32, A_RELOAD, 0, 0, 0), OK);
        stand(m, wr.reload_ticks);
    });
    assert_eq!(reloaded, fresh, "a reload breaks it");
    let swapped = after_brace("rifle", |m| {
        m.act(ME as u32, A_SWAP, 1, 0, 0);
        m.step();
        m.act(ME as u32, A_SWAP, 0, 0, 0);
        m.step();
    });
    assert_eq!(swapped, fresh, "a swap breaks it");
    let cancelled = after_brace("rifle", |m| {
        m.act(ME as u32, A_CANCEL, 0, 0, 0);
        m.step();
    });
    assert_eq!(cancelled, fresh, "a cancel breaks it");
}

#[test]
fn an_ability_use_breaks_the_brace() {
    let mut m = range(id("rifle"));
    assert!(weapon(&m, id("rifle")).cooldown >= 1);
    startup(&mut m);
    assert!(m.world.units[ME].combat.braced);
    // In the cooldown (where a refused pull keeps the brace), use the decoy of the shroud kit.
    m.world.units[ME].kit = 2;
    m.world.units[ME].abil.charges[0] = 1;
    let (x, y) = m.world.units[ME].pos;
    assert_eq!(m.act(ME as u32, A_ABILITY, 0, x + 3 * 1024, y), OK);
    m.step();
    assert!(!m.world.units[ME].combat.braced, "the cast took the hands off the trigger");
    assert_eq!(me(&m).state & S_BRACED, 0);
}

#[test]
fn an_empty_magazine_keeps_the_brace_until_the_reload() {
    let mut m = range(id("rifle"));
    let wr = weapon(&m, id("rifle"));
    m.world.units[ME].ammo[0] = 1;
    startup(&mut m);
    stand(&mut m, wr.cooldown + 3);
    assert!(
        m.world.units[ME].combat.braced,
        "nothing to fire: not ready, so not let go"
    );
    assert_eq!(combat::reload(&mut m.world, ME), OK);
    assert!(!m.world.units[ME].combat.braced);
}

#[test]
fn reported_velocity_is_the_distance_actually_moved() {
    // Up against the left wall, moving into it: the unit does not move and bots see 0.
    let mut m = range(id("pistol"));
    let r = m.world.rules.core.unit_radius;
    place(&mut m, ME, (TILE + r, at(4, 7).1), 0);
    m.act(ME as u32, A_MOVE, 32768, 500, 0);
    m.step();
    assert_eq!(m.world.units[ME].pos, (TILE + r, at(4, 7).1));
    assert_eq!((me(&m).vx, me(&m).vy), (0, 0));
    assert_eq!(
        m.world.units[ME].combat.unsteady, 0,
        "a blocked move does not unsteady the unit"
    );
    // Sliding along the wall: only the free component is reported.
    m.act(ME as u32, A_MOVE, 32768 + 4096, 500, 0);
    m.step();
    let v = (me(&m).vx, me(&m).vy);
    assert!(v.0 == 0 && v.1 < 0, "{v:?}");
    // In the open the full move.
    place(&mut m, ME, at(10, 7), 0);
    m.act(ME as u32, A_MOVE, 0, 500, 0);
    m.step();
    assert_eq!((me(&m).vx, me(&m).vy), (500, 0));
}
