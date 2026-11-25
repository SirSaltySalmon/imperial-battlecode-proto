//! Trigger pulls: wind-up, cooldown, the fan, recoil, and the release-tick speed rule.
//! Scripts follow `combat_firing`.

use crate::record::Catalog;
use crate::setup::{self, weapon_id};
use crate::take::{self, Take};
use ibc_engine::abi::*;
use ibc_engine::combat::fan;
use ibc_engine::fixed;
use ibc_engine::Ruleset;

pub fn add(cat: &mut Catalog) {
    let rules = Ruleset::builtin();
    let rifle = weapon_id("rifle");
    let shotgun = weapon_id("shotgun");
    let marksman = weapon_id("marksman");
    let pistol = weapon_id("pistol");

    one_pull(cat, marksman, "marksman-windup", "Marksman wind-up",
        "combat_firing::bullets_leave_after_the_wind_up_along_the_fan. Two ticks of wind-up, then one bullet on the aim line.");
    hold(cat, shotgun, 3, "shotgun-cycle", "Shotgun pump",
        "combat_firing::one_pull_takes_windup_plus_one_plus_cooldown_ticks. Wind-up, the release, then two ticks of cooldown before the next pull is accepted.");
    hold(cat, pistol, 4, "pistol-cycle", "Pistol, no cooldown",
        "Same clock with cooldown 0: the next pull is accepted the tick after the bullets leave, so a pull is wind-up + 1 ticks.");

    release_speed(cat, rifle, false, true, "rifle-move-on-release",
        "Rifle moves on the release tick",
        "combat_firing::spread_uses_the_speed_on_the_tick_the_bullets_leave. Strafing only as the bullets leave opens the fan.");
    release_speed(cat, rifle, true, false, "rifle-stop-on-release",
        "Rifle stops for the release tick",
        "Moving through the wind-up and stopping for the tick the bullets leave gives the standing fan.");

    aim_clamp(cat, pistol);
    recoil_walk(cat, rifle);
    for w in &rules.weapons {
        cat.hold.push(measure_hold(w.id));
    }
}

fn one_pull(cat: &mut Catalog, id: i32, sid: &str, title: &str, note: &str) {
    let mut t = Take::begin(setup::range(id));
    let windup = t.m.world.rules.weapons[id as usize].windup;
    t.act(0, A_FIRE, 0, 0);
    for _ in 0..windup + 6 {
        t.act(0, A_FACE, 0, 0);
        t.step();
    }
    let shots = t.shots(0);
    t.finish(
        cat,
        take::meta("pull", sid, title, note, &format!("shots {shots}")),
    );
}

fn hold(cat: &mut Catalog, id: i32, pulls: i32, sid: &str, title: &str, note: &str) {
    let mut t = Take::begin(setup::range(id));
    let mut guard = 0;
    while t.shots(0) < pulls && guard < 40 {
        t.act(0, A_FACE, 0, 0);
        t.act(0, A_FIRE, 0, 0);
        t.step();
        guard += 1;
    }
    for _ in 0..4 {
        t.step();
    }
    let shots = t.shots(0);
    t.finish(
        cat,
        take::meta("pull", sid, title, note, &format!("{shots} pulls")),
    );
}

fn release_speed(
    cat: &mut Catalog,
    id: i32,
    during: bool,
    at_release: bool,
    sid: &str,
    title: &str,
    note: &str,
) {
    let mut t = Take::begin(setup::range(id));
    t.act(0, A_FIRE, 0, 0);
    if during {
        t.act(0, A_MOVE, 16384, 630);
    }
    t.step();
    t.act(0, A_FACE, 0, 0);
    if at_release {
        t.act(0, A_MOVE, 16384, 630);
    }
    t.step();
    for _ in 0..3 {
        t.step();
    }
    let wr = t.m.world.rules.weapons[id as usize];
    let speed = if at_release { 630 } else { 0 };
    let spread = fan(&wr, 0, 0, t.m.world.rules.core.fov_half, speed, 0, 0).step;
    t.finish(
        cat,
        take::meta("fans", sid, title, note, &format!("fan step {spread}")),
    );
}

fn aim_clamp(cat: &mut Catalog, pistol: i32) {
    let mut t = Take::begin(setup::range(pistol));
    t.act(0, A_FACE, 0, 0);
    t.act(0, A_FIRE, 16384, 0);
    t.step();
    t.act(0, A_FACE, 0, 0);
    t.step();
    for _ in 0..3 {
        t.step();
    }
    let fov = t.m.world.rules.core.fov_half;
    t.finish(
        cat,
        take::meta(
            "fans",
            "pistol-aim-clamp",
            "Aim clamped to the view cone",
            "combat_firing::aim_is_clamped_to_the_view_cone. Facing +x, asking for a shot straight down. The bullet leaves along the edge of the cone.",
            &format!("bullet along the cone edge ({fov})"),
        ),
    );
}

fn recoil_walk(cat: &mut Catalog, rifle: i32) {
    let (t, samples) = drain(rifle);
    let peak = samples.iter().map(|s| s[0]).max().unwrap_or(0);
    t.finish(
        cat,
        take::meta(
            "fans",
            "rifle-recoil",
            "Rifle recoil walk",
            "combat_firing::recoil_builds_per_shot_recovers_on_quiet_ticks_and_widens_the_fan and recoil_shift_alternates_sides. Holding the trigger: each release adds heat, quiet ticks (wind-up and cooldown) recover it, and the leftover shoves the next fan to alternating sides.",
            &format!("peak heat {peak}"),
        ),
    );
}

/// Empty the magazine as fast as the weapon allows, then wait until recoil has settled.
fn measure_hold(id: i32) -> Vec<[i32; 3]> {
    drain(id).1
}

fn drain(id: i32) -> (Take, Vec<[i32; 3]>) {
    let mut t = Take::begin(setup::range(id));
    let wr = t.m.world.rules.weapons[id as usize];
    let fov = t.m.world.rules.core.fov_half;
    let mut samples = Vec::new();
    let limit = wr.magazine * (wr.windup + 1 + wr.cooldown) + 40;
    for _ in 0..limit {
        let heat = t.m.world.units[0].combat.recoil;
        let before = t.shots(0);
        t.act(0, A_FACE, 0, 0);
        t.act(0, A_FIRE, 0, 0);
        t.step();
        if t.shots(0) > before {
            let f = fan(&wr, 0, 0, fov, 0, heat, before);
            samples.push([heat, fixed::angle_diff(0, f.centre), f.step]);
        }
        if samples.len() as i32 == wr.magazine && t.m.world.units[0].combat.recoil == 0 {
            break;
        }
    }
    (t, samples)
}
