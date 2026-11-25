//! Combat lab: replay the combat tests and draw each weapon's fan.
//!
//! ```text
//! cargo run -p ibc-combatviz
//! ```
//!
//! writes `out/combat-lab/lab.json`, which the replay viewer's combat lab draws.

mod firing;
mod flight;
mod handling;
mod record;
mod setup;
mod take;
mod ttk;

use ibc_engine::abi::ITEM_KIND_ARMOR;
use ibc_engine::combat::{damage_at, fan};
use ibc_engine::Ruleset;
use record::{DamageCheck, FanCheck, Lab, WeaponOut};

pub fn build() -> Lab {
    let rules = Ruleset::builtin();
    let heavy = rules
        .items
        .iter()
        .filter(|item| item.kind == ITEM_KIND_ARMOR)
        .map(|item| item.value)
        .max()
        .unwrap_or(0);
    let mut cat = record::Catalog::new();
    ttk::add(&mut cat, heavy);
    firing::add(&mut cat);
    flight::add(&mut cat);
    handling::add(&mut cat);
    assert_eq!(
        cat.hold.len(),
        rules.weapons.len(),
        "one recoil trace per weapon"
    );
    let (checks, damage_checks) = checks(&rules);
    Lab {
        tile: ibc_engine::abi::TILE,
        radius: rules.core.unit_radius,
        fov_half: rules.core.fov_half,
        max_hp: rules.core.max_hp,
        run_speed: rules.core.run_speed,
        heavy_armor: heavy,
        vision_range: rules.core.vision_range,
        sense_radius: rules.core.sense_radius,
        ruleset: format!("{:016x}", rules.hash()),
        weapons: weapons(&rules, &cat.hold),
        maps: cat.maps,
        scenes: cat.scenes,
        checks,
        damage_checks,
    }
}

fn weapons(rules: &Ruleset, hold: &[Vec<[i32; 2]>]) -> Vec<WeaponOut> {
    rules
        .weapons
        .iter()
        .zip(&rules.weapon_names)
        .enumerate()
        .map(|(i, (w, name))| WeaponOut {
            name: name.clone(),
            id: w.id,
            slot: w.slot,
            price: w.price,
            damage: w.damage,
            pellets: w.pellets,
            spread: w.spread,
            recoil_spread: w.recoil_spread,
            recoil_per_shot: w.recoil_per_shot,
            recoil_recovery: w.recoil_recovery,
            windup: w.windup,
            cooldown: w.cooldown,
            magazine: w.magazine,
            reload_ticks: w.reload_ticks,
            bullet_speed: w.bullet_speed,
            range: w.range,
            falloff_start: w.falloff_start,
            falloff_min_pct: w.falloff_min_pct,
            move_pct: w.move_pct,
            kill_reward: w.kill_reward,
            unsteady_gain: w.unsteady_gain,
            unsteady_decay: w.unsteady_decay,
            unsteady_max: w.unsteady_max,
            startup_per_point: w.startup_per_point,
            top_speed: rules.core.run_speed * w.move_pct / 100,
            hold: hold[i].clone(),
        })
        .collect()
}

fn checks(rules: &Ruleset) -> (Vec<FanCheck>, Vec<DamageCheck>) {
    let fov = rules.core.fov_half;
    let mut fans = Vec::new();
    let mut damage = Vec::new();
    for w in &rules.weapons {
        for (facing, aim, recoil) in [(0, 0, 0), (0, 0, 1), (0, 0, 4), (1000, 1500, 3)] {
            record_fan(&mut fans, w, fov, (facing, aim, recoil));
        }
        for travelled in [0, w.falloff_start, (w.falloff_start + w.range) / 2, w.range] {
            damage.push(DamageCheck {
                weapon: w.id,
                travelled,
                damage: damage_at(w, travelled),
            });
        }
    }
    let pistol = &rules.weapons[0];
    record_fan(&mut fans, pistol, fov, (5000, 5000 + fov + 4000, 0));
    record_fan(&mut fans, pistol, fov, (5000, 5000 - fov - 4000, 0));
    (fans, damage)
}

fn record_fan(
    out: &mut Vec<FanCheck>,
    w: &ibc_engine::abi::WeaponRules,
    fov: i32,
    sample: (i32, i32, i32),
) {
    let (facing, aim, recoil) = sample;
    let f = fan(w, facing, aim, fov, recoil);
    out.push(FanCheck {
        weapon: w.id,
        facing,
        aim,
        recoil,
        centre: f.centre,
        step: f.step,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene<'a>(lab: &'a Lab, id: &str) -> &'a record::Scene {
        lab.scenes
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("missing scene {id}"))
    }

    #[test]
    fn the_lab_carries_the_rules_the_viewer_draws_with() {
        let lab = build();
        assert!(lab.vision_range > 0 && lab.sense_radius > 0 && lab.radius > 0);
        assert_eq!(
            lab.weapons.len(),
            lab.checks.iter().map(|c| c.weapon).max().unwrap() as usize + 1
        );
    }

    #[test]
    fn ttk_outcomes_are_the_tick_the_target_dies() {
        let lab = build();
        let ttk: Vec<_> = lab.scenes.iter().filter(|s| s.ttk.is_some()).collect();
        assert_eq!(ttk.len(), lab.weapons.len() * 3 * 2 * 2);
        for s in ttk {
            let died = s.frames.iter().position(|f| f.u[1][4] == 0);
            // A running duel records its run-in before the first pull starts the clock.
            let run_in = if s.ttk.as_ref().unwrap().running {
                ttk::RUN_IN as usize
            } else {
                0
            };
            let expect = died
                .map(|i| (i - run_in).to_string())
                .unwrap_or_else(|| "no kill".to_string());
            assert_eq!(s.outcome, expect, "{}", s.id);
        }
    }

    #[test]
    fn wall_shots_miss_and_the_open_lane_hits() {
        let lab = build();
        for s in lab.scenes.iter().filter(|s| s.id.starts_with("wall-")) {
            assert_eq!(s.outcome, "dealt 0", "{}", s.id);
        }
        assert!(!scene(&lab, "open-lane").outcome.starts_with("dealt 0"));
    }

    #[test]
    fn a_strafe_dodges_and_a_lead_hits() {
        let lab = build();
        for name in ["pistol", "marksman"] {
            assert_eq!(
                scene(&lab, &format!("dodge-{name}-15-strafe")).outcome,
                "dealt 0"
            );
            assert!(!scene(&lab, &format!("dodge-{name}-15-stand"))
                .outcome
                .starts_with("dealt 0"));
            assert!(!scene(&lab, &format!("dodge-{name}-15-lead"))
                .outcome
                .starts_with("dealt 0"));
        }
    }

    #[test]
    fn range_edge_trade_cancel_and_drops() {
        let lab = build();
        assert!(!scene(&lab, "marksman-inside")
            .outcome
            .starts_with("dealt 0"));
        assert!(scene(&lab, "marksman-past").outcome.starts_with("dealt 0"));
        assert_eq!(scene(&lab, "trade").outcome, "both dead");
        assert!(scene(&lab, "marksman-cancel")
            .outcome
            .starts_with("shots 0"));
        assert_eq!(scene(&lab, "killed-windup").outcome, "shots 0");

        let swap = scene(&lab, "swap-cooldown");
        assert!(swap.frames.iter().any(|f| f.u[0][7] == 0 && f.u[0][12] > 0));

        let death = scene(&lab, "death-drops");
        let last = death.frames.last().unwrap();
        assert_eq!(last.d.len(), 1);
        assert_eq!(last.d[0][2], 1, "the rifle");

        let picked = scene(&lab, "pickup-empty");
        let u = &picked.frames.last().unwrap().u[3];
        assert_eq!(
            (u[16], u[17], u[7]),
            (1, 0, 0),
            "rifle empty, pistol still in hand"
        );
    }
}
