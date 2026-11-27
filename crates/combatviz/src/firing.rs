//! Trigger pulls: startup, the brace, cooldown, the fan, recoil, and unsteadiness after moving.
//! Scripts follow `combat_firing` and `combat_brace`.

use crate::record::Catalog;
use crate::setup::{self, weapon_id};
use crate::take::{self, Take};
use ibc_engine::abi::*;
use ibc_engine::combat::{fan, unsteadiness_after};
use ibc_engine::Ruleset;

pub fn add(cat: &mut Catalog) {
    let rules = Ruleset::builtin();
    let rifle = weapon_id("rifle");
    let shotgun = weapon_id("shotgun");
    let marksman = weapon_id("marksman");
    let pistol = weapon_id("pistol");

    let windup = rules.weapons[marksman as usize].windup;
    one_pull(cat, marksman, "marksman-windup", "Marksman startup",
        &format!("combat_firing::bullets_leave_after_the_startup_along_the_fan. A steady unit starts up for {windup} ticks, then one bullet leaves on the aim line."));
    hold(cat, shotgun, 3, "shotgun-cycle", "Shotgun pump, braced",
        "combat_firing::holding_the_trigger_fires_after_the_startup_then_every_one_plus_cooldown_ticks. The startup, the release, then braced: one pull every 1 + cooldown ticks.");
    let cooldown = rules.weapons[pistol as usize].cooldown;
    hold(cat, pistol, 4, "pistol-cycle", "Pistol, braced",
        &format!("The same clock with cooldown {cooldown}: braced, a pull every {} ticks leaves on the tick it is made.", 1 + cooldown));

    let rifle_rules = &rules.weapons[rifle as usize];
    let run = rules.core.run_speed * rifle_rules.move_pct / 100;
    let walked = 3;
    let ran = ticks_to_cap(rifle_rules, run);
    startup_after(cat, rifle, 0, 0, "rifle-startup-steady", "Rifle, steady",
        "combat_brace::running_then_firing_starts_up_slower_than_walking_than_standing. Standing settled: the startup is the wind-up.");
    startup_after(
        cat,
        rifle,
        rules.core.walk_speed,
        walked,
        "rifle-startup-walked",
        "Rifle after walking",
        &format!(
            "{walked} ticks of walking leave a little unsteadiness: a slightly longer startup."
        ),
    );
    startup_after(cat, rifle, run, ran, "rifle-startup-ran", "Rifle after running",
        &format!("{ran} ticks of running reach the cap of {}: the longest startup. Stopping for one tick first would barely shorten it.", rifle_rules.unsteady_max));
    move_cancels(cat, marksman);

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

/// Ticks of moving `speed` units per tick from a standstill until unsteadiness reaches `unsteady_max`.
fn ticks_to_cap(wr: &WeaponRules, speed: i32) -> i32 {
    let (mut heat, mut ticks) = (0, 0);
    while heat < wr.unsteady_max && ticks < 64 {
        heat = unsteadiness_after(wr, heat, speed);
        ticks += 1;
    }
    ticks
}

/// Move at `speed` for `ticks` ticks (legs of three ticks up and down), then pull and hold the trigger.
fn startup_after(
    cat: &mut Catalog,
    id: i32,
    speed: i32,
    ticks: i32,
    sid: &str,
    title: &str,
    note: &str,
) {
    let mut t = Take::begin(setup::range(id));
    for k in 0..ticks {
        let angle = if (k / 3) % 2 == 0 { 16384 } else { 49152 };
        t.act(0, A_MOVE, angle, speed);
        t.step();
    }
    let heat = t.m.world.units[0].combat.unsteady;
    let mut startup = 0;
    while t.shots(0) == 0 && startup < 20 {
        t.act(0, A_FACE, 0, 0);
        t.act(0, A_FIRE, 0, 0);
        t.step();
        startup += 1;
    }
    for _ in 0..3 {
        t.step();
    }
    t.finish(
        cat,
        take::meta(
            "pull",
            sid,
            title,
            note,
            &format!("unsteadiness {heat}, startup {}", startup - 1),
        ),
    );
}

fn move_cancels(cat: &mut Catalog, id: i32) {
    let mut t = Take::begin(setup::range(id));
    t.act(0, A_FIRE, 0, 0);
    t.step();
    t.act(0, A_MOVE, 16384, 300);
    t.step();
    for _ in 0..4 {
        t.step();
    }
    let shots = t.shots(0);
    t.finish(
        cat,
        take::meta(
            "pull",
            "marksman-move-cancels",
            "A move cancels the startup",
            "combat_brace::a_move_cancels_a_startup_and_a_pull_on_a_moving_tick_is_refused. The marksman pulls, then steps aside during its startup: no bullet, nothing spent.",
            &format!("shots {shots}"),
        ),
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
            "combat_firing::recoil_builds_per_shot_recovers_on_quiet_ticks_and_widens_the_fan and sustained_fire_widens_the_fan_but_never_moves_it_off_the_aim. Holding the trigger braced: each release adds heat, cooldown ticks recover it, and the leftover opens the fan around the aim.",
            &format!("peak heat {peak}"),
        ),
    );
}

/// Empty the magazine as fast as the weapon allows, then wait until recoil has settled.
fn measure_hold(id: i32) -> Vec<[i32; 2]> {
    drain(id).1
}

fn drain(id: i32) -> (Take, Vec<[i32; 2]>) {
    let mut t = Take::begin(setup::range(id));
    let wr = t.m.world.rules.weapons[id as usize];
    let fov = t.m.world.rules.core.fov_half;
    let mut samples = Vec::new();
    let limit = wr.windup + wr.magazine * (1 + wr.cooldown) + 40;
    for _ in 0..limit {
        let heat = t.m.world.units[0].combat.recoil;
        let before = t.shots(0);
        t.act(0, A_FACE, 0, 0);
        t.act(0, A_FIRE, 0, 0);
        t.step();
        if t.shots(0) > before {
            samples.push([heat, fan(&wr, 0, 0, fov, heat).step]);
        }
        if samples.len() as i32 == wr.magazine && t.m.world.units[0].combat.recoil == 0 {
            break;
        }
    }
    (t, samples)
}
