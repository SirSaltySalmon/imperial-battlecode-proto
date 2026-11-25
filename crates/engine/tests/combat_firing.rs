//! Trigger pulls: the fan maths, wind-up and cooldown timing, recoil, magazines, reload and cancel edge cases.

mod combat_common;

use combat_common::*;
use ibc_engine::abi::*;
use ibc_engine::combat::{self, fan};
use ibc_engine::{fixed, Match, Ruleset};

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

fn all_weapons() -> Vec<i32> {
    (0..Ruleset::builtin().weapons.len() as i32).collect()
}

/// Step until the unit has pulled the trigger `n` times in total; returns the steps taken.
fn step_until_shots(m: &mut Match, n: i32) -> i32 {
    let mut steps = 0;
    while shots(m, ME) < n {
        m.step();
        steps += 1;
        assert!(steps < 50, "no shot after {steps} ticks");
    }
    steps
}

#[test]
fn the_ruleset_has_a_weapon_for_every_role() {
    let r = Ruleset::builtin();
    for name in ["pistol", "rifle", "smg", "shotgun", "marksman", "lmg"] {
        weapon_id(&r, name);
    }
    // Ids the shop and the default loadout refer to must not move.
    assert_eq!((weapon_id(&r, "pistol"), weapon_id(&r, "rifle")), (0, 1));
    assert_eq!(r.weapons[0].slot, 1, "the default sidearm is a sidearm");
    for (w, name) in r.weapons.iter().zip(&r.weapon_names) {
        assert!(
            w.damage > 0 && w.pellets >= 1 && w.magazine >= 1 && w.reload_ticks >= 1,
            "{name}"
        );
        assert!(
            w.windup >= 1,
            "{name}: a zero wind-up gives the target nothing to react to"
        );
        assert!(
            w.cooldown >= 0 && w.spread >= 0 && w.move_spread >= 0 && w.recoil_spread >= 0,
            "{name}"
        );
        assert!(
            w.recoil_per_shot >= 0 && w.recoil_recovery >= 1,
            "{name}: recoil must settle"
        );
        assert!(
            w.range > 0 && w.falloff_start <= w.range && (0..=100).contains(&w.falloff_min_pct),
            "{name}"
        );
        assert!((50..=100).contains(&w.move_pct), "{name}");
        // The core sweeps bullets per movement slice; a slice longer than a tile is still exact (swept), but
        // keep bullets slower than the map is wide so that "travelling bullets" stays true.
        assert!(w.bullet_speed <= 24 * TILE, "{name}");
    }
}

#[test]
fn settled_standing_fan_is_centred_on_the_aim() {
    let r = Ruleset::builtin();
    for w in &r.weapons {
        let f = fan(w, 1000, 1000, r.core.fov_half, 0, 0, 0);
        assert_eq!((f.centre, f.step, f.pellets), (1000, w.spread, w.pellets));
        let angles: Vec<i32> = f.angles().collect();
        // Symmetric about the aim, `spread` apart, total width (pellets - 1) * spread.
        for (i, a) in angles.iter().enumerate() {
            let mirror = angles[angles.len() - 1 - i];
            assert_eq!(
                fixed::angle_diff(1000, *a),
                -fixed::angle_diff(1000, mirror),
                "weapon {}",
                w.id
            );
        }
        let width = fixed::angle_diff(angles[0], angles[angles.len() - 1]);
        assert!(
            (width - (w.pellets - 1) * w.spread).abs() <= 1,
            "weapon {}: width {width}",
            w.id
        );
        if w.pellets % 2 == 1 {
            assert_eq!(
                angles[angles.len() / 2],
                1000,
                "odd fans put a bullet on the aim line"
            );
        }
    }
}

#[test]
fn fan_numbers_for_a_running_rifle() {
    let r = Ruleset::builtin();
    let rifle = r.weapons[weapon_id(&r, "rifle") as usize];
    assert_eq!(
        (
            rifle.spread,
            rifle.move_spread,
            rifle.recoil_spread,
            rifle.pellets
        ),
        (250, 1200, 250, 3)
    );
    // speed 630: extra = 1200 * 630 / 1024 = 738; S = 988; shift = 369, right on even shots, left on odd.
    let f = fan(&rifle, 0, 0, r.core.fov_half, 630, 0, 0);
    assert_eq!((f.centre, f.step), (369, 988));
    assert_eq!(
        f.angles().collect::<Vec<_>>(),
        vec![65536 + 369 - 988, 369, 369 + 988]
    );
    let f = fan(&rifle, 0, 0, r.core.fov_half, 630, 0, 1);
    assert_eq!((f.centre, f.step), (65536 - 369, 988));
    // Recoil adds recoil_spread per point: 2 points -> extra 500, S = 750, shift 250.
    let f = fan(&rifle, 0, 0, r.core.fov_half, 0, 2, 0);
    assert_eq!((f.centre, f.step), (250, 750));
    // Both together add up.
    let f = fan(&rifle, 0, 0, r.core.fov_half, 630, 2, 0);
    assert_eq!((f.centre, f.step), ((738 + 500) / 2, 250 + 738 + 500));
}

#[test]
fn standing_fire_is_tighter_than_running_fire() {
    let r = Ruleset::builtin();
    for w in &r.weapons {
        let run = r.core.run_speed * w.move_pct / 100;
        let mut last = (-1, -1);
        for speed in [0, r.core.walk_speed, run] {
            let f = fan(w, 0, 0, r.core.fov_half, speed, 0, 0);
            let miss = fixed::angle_diff(0, f.centre).abs();
            assert!(
                f.step > last.0 && miss > last.1,
                "weapon {} at speed {speed}: {f:?}",
                w.id
            );
            last = (f.step, miss);
        }
    }
}

#[test]
fn aim_is_clamped_to_the_view_cone() {
    let r = Ruleset::builtin();
    let fov = r.core.fov_half;
    let pistol = r.weapons[0];
    assert_eq!(
        fan(&pistol, 5000, 5000 + fov + 4000, fov, 0, 0, 0).centre,
        5000 + fov
    );
    assert_eq!(
        fan(&pistol, 5000, 5000 - fov - 4000, fov, 0, 0, 0).centre,
        (5000 - fov) & 0xFFFF
    );
    assert_eq!(
        fan(&pistol, 5000, 5000 + fov - 1, fov, 0, 0, 0).centre,
        5000 + fov - 1
    );

    // In the world: look along +x, keep facing there, ask for a shot straight down.
    let mut m = range(0);
    m.act(ME as u32, A_FACE, 0, 0, 0);
    m.act(ME as u32, A_FIRE, 16384, 0, 0);
    m.step();
    m.act(ME as u32, A_FACE, 0, 0, 0);
    m.step();
    assert_eq!(m.world.projectiles.len(), 1);
    assert_eq!(
        m.world.projectiles[0].vel,
        fixed::polar(fov, pistol.bullet_speed)
    );
}

#[test]
fn bullets_leave_after_the_wind_up_along_the_fan() {
    for id in all_weapons() {
        let mut m = range(id);
        let wr = weapon(&m, id);
        assert_eq!(m.act(ME as u32, A_FIRE, 300, 0, 0), OK);
        for tick in 0..wr.windup {
            m.step();
            assert_eq!(shots(&m, ME), 0, "weapon {id} fired {tick} ticks early");
            assert!(m.world.units[ME].combat.winding);
            assert_eq!(m.world.units[ME].combat.aim, 300);
            assert_eq!(
                m.world.units[ME].combat.channel(),
                wr.windup - tick,
                "ticks left, as bots see it"
            );
        }
        let origin = m.world.units[ME].pos;
        m.step();
        assert_eq!(shots(&m, ME), 1, "weapon {id}");
        assert_eq!(
            m.world.units[ME].ammo[m.world.units[ME].slot],
            wr.magazine - 1
        );
        let expect: Vec<_> = fan(&wr, 300, 300, m.world.rules.core.fov_half, 0, 0, 0)
            .angles()
            .map(|a| fixed::polar(a, wr.bullet_speed))
            .collect();
        let got: Vec<_> = m.world.projectiles.iter().map(|p| p.vel).collect();
        assert_eq!(got, expect, "weapon {id}");
        for p in &m.world.projectiles {
            assert_eq!((p.owner, p.weapon, p.range), (ME as u32, id, wr.range));
            // One tick of flight from the rim of the shooter's circle.
            let flown = fixed::dist(origin, p.pos) - m.world.rules.core.unit_radius;
            assert!(
                (flown - wr.bullet_speed).abs() <= 16
                    && (p.travelled - wr.bullet_speed).abs() <= 16
            );
        }
    }
}

#[test]
fn spread_uses_the_speed_on_the_tick_the_bullets_leave() {
    let id = weapon_id(&Ruleset::builtin(), "rifle");
    let wr = Ruleset::builtin().weapons[id as usize];
    let fov = Ruleset::builtin().core.fov_half;
    let vels = |move_during_windup: bool, move_at_release: bool| {
        let mut m = range(id);
        m.act(ME as u32, A_FIRE, 0, 0, 0);
        if move_during_windup {
            m.act(ME as u32, A_MOVE, 16384, 630, 0);
        }
        m.step();
        m.act(ME as u32, A_FACE, 0, 0, 0);
        if move_at_release {
            m.act(ME as u32, A_MOVE, 16384, 630, 0);
        }
        m.step();
        m.world
            .projectiles
            .iter()
            .map(|p| p.vel)
            .collect::<Vec<_>>()
    };
    let of = |speed: i32| {
        fan(&wr, 0, 0, fov, speed, 0, 0)
            .angles()
            .map(|a| fixed::polar(a, wr.bullet_speed))
            .collect::<Vec<_>>()
    };
    assert_eq!(vels(false, false), of(0));
    assert_eq!(
        vels(true, false),
        of(0),
        "stopping for the release tick gives the standing fan"
    );
    assert_eq!(vels(false, true), of(630));
    assert_eq!(vels(true, true), of(630));
}

#[test]
fn one_pull_takes_windup_plus_one_plus_cooldown_ticks() {
    for id in all_weapons() {
        let mut m = range(id);
        let wr = weapon(&m, id);
        let pulls = wr.magazine.min(4);
        let mut release_ticks = Vec::new();
        for tick in 0..60 {
            let ready = combat::can_fire(&m.world, ME) == OK;
            assert_eq!(
                m.act(ME as u32, A_FIRE, 0, 0, 0) == OK,
                ready,
                "act agrees with can_fire"
            );
            let before = shots(&m, ME);
            m.step();
            if shots(&m, ME) > before {
                release_ticks.push(tick);
                // The tick after a release is ready only if there is no cooldown.
                assert_eq!(
                    combat::can_fire(&m.world, ME) == OK,
                    wr.cooldown == 0,
                    "weapon {id}"
                );
                assert_eq!(m.world.units[ME].combat.cooldown, wr.cooldown);
            }
            if release_ticks.len() as i32 == pulls {
                break;
            }
        }
        let expect: Vec<i32> = (0..pulls)
            .map(|k| wr.windup + k * (wr.windup + 1 + wr.cooldown))
            .collect();
        assert_eq!(release_ticks, expect, "weapon {id}");
    }
}

#[test]
fn recoil_builds_per_shot_recovers_on_quiet_ticks_and_widens_the_fan() {
    for id in all_weapons() {
        let mut m = range(id);
        let wr = weapon(&m, id);
        let fov = m.world.rules.core.fov_half;
        let (mut model, mut peak) = (0, 0);
        let mut steps = Vec::new();
        // Empty the magazine as fast as the weapon allows, then wait.
        for _ in 0..(wr.magazine * (wr.windup + 1 + wr.cooldown) + 40) {
            m.act(ME as u32, A_FIRE, 0, 0, 0);
            m.act(ME as u32, A_FACE, 0, 0, 0);
            let before = shots(&m, ME);
            let releasing =
                m.world.units[ME].combat.winding && m.world.units[ME].combat.windup == 0;
            if releasing {
                steps.push(fan(&wr, 0, 0, fov, 0, model, before).step);
            }
            m.step();
            if shots(&m, ME) > before {
                assert!(releasing);
                model += wr.recoil_per_shot;
            } else {
                model = (model - wr.recoil_recovery).max(0);
            }
            assert_eq!(m.world.units[ME].combat.recoil, model, "weapon {id}");
            peak = peak.max(model);
        }
        assert_eq!(shots(&m, ME), wr.magazine);
        assert_eq!(peak >= wr.recoil_per_shot, true);
        assert_eq!(model, 0, "weapon {id}: recoil settles completely");
        assert_eq!(steps[0], wr.spread, "first shot is the settled fan");
        // Recovery between pulls is recovery * (ticks per pull - 1); if a pull adds more than that, heat
        // builds up and every fan is wider than the one before.
        let net = wr.recoil_per_shot - wr.recoil_recovery * (wr.windup + wr.cooldown);
        if net > 0 && wr.recoil_spread > 0 {
            assert!(
                steps.windows(2).all(|p| p[1] > p[0]),
                "weapon {id}: {steps:?}"
            );
        } else {
            assert!(
                steps.iter().all(|s| *s == wr.spread),
                "weapon {id}: {steps:?}"
            );
        }
        // Settled again: the next shot is as tight as the first.
        m.world.units[ME].ammo = [wr.magazine; 2];
        m.act(ME as u32, A_FIRE, 0, 0, 0);
        step_until_shots(&mut m, wr.magazine + 1);
        let n = wr.pellets as usize;
        let vels: Vec<_> = m.world.projectiles[m.world.projectiles.len() - n..]
            .iter()
            .map(|p| p.vel)
            .collect();
        // `shots` is odd or even, but with no extra spread the shift is zero either way.
        let expect: Vec<_> = fan(&wr, 0, 0, fov, 0, 0, 0)
            .angles()
            .map(|a| fixed::polar(a, wr.bullet_speed))
            .collect();
        assert_eq!(vels, expect, "weapon {id}");
    }
}

#[test]
fn recoil_shift_alternates_sides() {
    let id = weapon_id(&Ruleset::builtin(), "rifle");
    let mut m = range(id);
    let wr = weapon(&m, id);
    let mut centres = Vec::new();
    for _ in 0..3 {
        let n = shots(&m, ME);
        m.act(ME as u32, A_FIRE, 0, 0, 0);
        step_until_shots(&mut m, n + 1);
        let mid = m.world.projectiles[m.world.projectiles.len() - 2].vel;
        centres.push(fixed::angle_diff(0, fixed::atan2(mid.1, mid.0)));
    }
    assert_eq!(centres[0], 0);
    assert!(
        centres[1] < -100 && centres[2] > 100,
        "{centres:?} (per shot {})",
        wr.recoil_per_shot
    );
}

#[test]
fn magazine_runs_dry_and_reload_takes_reload_ticks() {
    for id in all_weapons() {
        let mut m = range(id);
        let wr = weapon(&m, id);
        let slot = wr.slot as usize;
        assert_eq!(
            combat::reload(&mut m.world, ME),
            ERR_NOT_READY,
            "full magazine"
        );
        m.world.units[ME].ammo[slot] = 1;
        m.act(ME as u32, A_FIRE, 0, 0, 0);
        step_until_shots(&mut m, 1);
        for _ in 0..wr.cooldown + 1 {
            m.step();
        }
        assert_eq!(m.world.units[ME].ammo[slot], 0);
        assert_eq!(m.act(ME as u32, A_FIRE, 0, 0, 0), ERR_NOT_READY, "empty");

        assert_eq!(m.act(ME as u32, A_RELOAD, 0, 0, 0), OK);
        for tick in 0..wr.reload_ticks {
            assert_eq!(
                m.world.units[ME].ammo[slot], 0,
                "weapon {id}: filled {tick} ticks in"
            );
            assert_eq!(
                m.act(ME as u32, A_FIRE, 0, 0, 0),
                ERR_NOT_READY,
                "weapon {id}: fired mid-reload"
            );
            m.step();
            assert_eq!(
                m.world.units[ME].combat.channel(),
                wr.reload_ticks - 1 - tick
            );
            if tick + 1 < wr.reload_ticks {
                assert_eq!(
                    combat::reload(&mut m.world, ME),
                    ERR_NOT_READY,
                    "a running reload does not restart"
                );
            }
        }
        assert_eq!(m.world.units[ME].ammo[slot], wr.magazine);
        assert_eq!(
            m.act(ME as u32, A_FIRE, 0, 0, 0),
            OK,
            "weapon {id}: ready the tick the reload ends"
        );
    }
}

#[test]
fn cancel_and_reload_abandon_a_wind_up_without_spending_ammo() {
    let id = weapon_id(&Ruleset::builtin(), "marksman");
    let wr = Ruleset::builtin().weapons[id as usize];
    assert!(wr.windup >= 2);

    // A_CANCEL mid wind-up: no bullet, no ammo, no cooldown, no recoil; ready again next tick.
    let mut m = range(id);
    m.act(ME as u32, A_FIRE, 0, 0, 0);
    m.step();
    m.act(ME as u32, A_CANCEL, 0, 0, 0);
    m.step();
    for _ in 0..wr.windup + 2 {
        assert_eq!(combat::can_fire(&m.world, ME), OK);
        m.step();
    }
    let u = &m.world.units[ME];
    assert_eq!(
        (shots(&m, ME), u.ammo[0], u.combat.recoil, u.combat.winding),
        (0, wr.magazine, 0, false)
    );
    assert!(m.world.projectiles.is_empty());

    // A_RELOAD mid wind-up (magazine not full): the shot is abandoned, the reload runs.
    let mut m = range(id);
    m.world.units[ME].ammo[0] = 2;
    m.act(ME as u32, A_FIRE, 0, 0, 0);
    m.step();
    m.act(ME as u32, A_RELOAD, 0, 0, 0);
    m.step();
    assert!(
        !m.world.units[ME].combat.winding && m.world.units[ME].combat.reload == wr.reload_ticks - 1
    );
    for _ in 0..wr.reload_ticks {
        m.step();
    }
    assert_eq!((shots(&m, ME), m.world.units[ME].ammo[0]), (0, wr.magazine));

    // A_CANCEL mid reload: the magazine keeps what it had.
    let mut m = range(id);
    m.world.units[ME].ammo[0] = 2;
    m.act(ME as u32, A_RELOAD, 0, 0, 0);
    m.step();
    m.act(ME as u32, A_CANCEL, 0, 0, 0);
    m.step();
    for _ in 0..wr.reload_ticks + 1 {
        m.step();
    }
    assert_eq!(
        (m.world.units[ME].ammo[0], m.world.units[ME].combat.reload),
        (2, 0)
    );
    assert_eq!(m.act(ME as u32, A_FIRE, 0, 0, 0), OK);
}

#[test]
fn swapping_abandons_reload_and_wind_up_and_keeps_cooldown() {
    let r = Ruleset::builtin();
    let (marksman, pistol) = (weapon_id(&r, "marksman"), weapon_id(&r, "pistol"));
    let wr = r.weapons[marksman as usize];
    let mut m = range(marksman);
    assert_eq!(m.world.units[ME].weapons, [marksman, pistol]);

    // Reload the primary, swap away half way: the primary stays as empty as it was.
    m.world.units[ME].ammo[0] = 1;
    m.act(ME as u32, A_RELOAD, 0, 0, 0);
    m.step();
    assert_eq!(m.act(ME as u32, A_SWAP, 1, 0, 0), OK);
    m.step();
    assert_eq!(
        (m.world.units[ME].slot, m.world.units[ME].combat.reload),
        (1, 0)
    );
    assert_eq!(
        m.act(ME as u32, A_FIRE, 0, 0, 0),
        OK,
        "the tick after a swap the new weapon is ready"
    );
    for _ in 0..wr.reload_ticks + 2 {
        m.step();
    }
    assert_eq!(m.world.units[ME].ammo[0], 1);

    // Swap during a wind-up: no shot.
    let n = shots(&m, ME);
    m.act(ME as u32, A_SWAP, 0, 0, 0);
    m.step();
    m.act(ME as u32, A_FIRE, 0, 0, 0);
    m.step();
    m.act(ME as u32, A_SWAP, 1, 0, 0);
    for _ in 0..4 {
        m.step();
    }
    assert_eq!((shots(&m, ME), m.world.units[ME].ammo[0]), (n, 1));

    // Fire the primary and swap at once: the sidearm waits out the primary's cooldown.
    m.act(ME as u32, A_SWAP, 0, 0, 0);
    m.step();
    m.act(ME as u32, A_FIRE, 0, 0, 0);
    step_until_shots(&mut m, n + 1);
    assert_eq!(m.world.units[ME].combat.cooldown, wr.cooldown);
    m.act(ME as u32, A_SWAP, 1, 0, 0);
    m.step();
    assert_eq!(m.world.units[ME].combat.cooldown, wr.cooldown - 1);
    // Swapping to the weapon already in hand changes nothing, wind-up included.
    while combat::can_fire(&m.world, ME) != OK {
        m.step();
    }
    m.act(ME as u32, A_FIRE, 0, 0, 0);
    m.step();
    m.act(ME as u32, A_SWAP, 1, 0, 0);
    m.step();
    assert_eq!(shots(&m, ME), n + 2);
}

#[test]
fn a_unit_killed_during_its_wind_up_does_not_fire() {
    let id = weapon_id(&Ruleset::builtin(), "marksman");
    let mut m = range(id);
    m.act(ME as u32, A_FIRE, 0, 0, 0);
    m.step();
    m.world.damage(ME, 1000, None, -1);
    assert!(!m.world.units[ME].alive && !m.world.units[ME].combat.winding);
    for _ in 0..4 {
        m.step();
    }
    assert_eq!(shots(&m, ME), 0);
    let _ = FOE;
}
