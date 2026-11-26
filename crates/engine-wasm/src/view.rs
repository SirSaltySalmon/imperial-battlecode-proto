//! JSON the viewer's panels read that the engine's own exports do not carry: the whole ruleset (every
//! weapon, ability, kit, shop item and economy number, for the buy menu) and each unit's loadout per frame
//! (both weapon slots, ammo, ability charges, for the team tab). Viewer-only output: nothing here feeds back
//! into the simulation.

use ibc_engine::{Ruleset, World};
use serde_json::{json, Value};

/// `ibc_engine::rules_json` plus `core`, `economy`, `weapon_rules`, `ability_rules`, `kit_rules` and
/// `item_rules`. Every record carries its `id` and `name`; the other fields are the ruleset's own names.
pub fn rules(r: &Ruleset) -> String {
    let mut v: Value =
        serde_json::from_str(&ibc_engine::rules_json(r)).unwrap_or_else(|_| json!({}));
    v["core"] = core(r);
    v["economy"] = economy(r);
    v["weapon_rules"] = r
        .weapons
        .iter()
        .zip(&r.weapon_names)
        .map(|(w, name)| {
            json!({
                "id": w.id, "name": name, "slot": w.slot, "price": w.price, "damage": w.damage,
                "pellets": w.pellets, "windup": w.windup, "cooldown": w.cooldown, "magazine": w.magazine,
                "reload_ticks": w.reload_ticks, "range": w.range, "falloff_start": w.falloff_start,
                "falloff_min_pct": w.falloff_min_pct, "bullet_speed": w.bullet_speed, "spread": w.spread,
                "recoil_spread": w.recoil_spread,
                "recoil_per_shot": w.recoil_per_shot, "recoil_recovery": w.recoil_recovery,
                "move_pct": w.move_pct, "kill_reward": w.kill_reward,
                "unsteady_gain": w.unsteady_gain, "unsteady_decay": w.unsteady_decay,
                "unsteady_max": w.unsteady_max, "startup_per_point": w.startup_per_point,
            })
        })
        .collect();
    v["ability_rules"] = r
        .abilities
        .iter()
        .zip(&r.ability_names)
        .map(|(a, name)| {
            json!({
                "id": a.id, "name": name, "effect": a.effect, "price": a.price, "max_charges": a.max_charges,
                "cooldown": a.cooldown, "range": a.range, "radius": a.radius, "duration": a.duration,
                "power": a.power, "delay": a.delay, "speed": a.speed,
            })
        })
        .collect();
    v["kit_rules"] = r
        .kits
        .iter()
        .zip(&r.kit_names)
        .map(|(k, name)| {
            json!({ "id": k.id, "name": name, "abilities": k.abilities, "hp": k.hp, "speed_pct": k.speed_pct })
        })
        .collect();
    v["item_rules"] = r
        .items
        .iter()
        .zip(&r.item_names)
        .map(|(i, name)| json!({ "id": i.id, "name": name, "kind": i.kind, "price": i.price, "value": i.value }))
        .collect();
    v.to_string()
}

fn core(r: &Ruleset) -> Value {
    let c = &r.core;
    json!({
        "team_size": c.team_size, "rounds_to_win": c.rounds_to_win, "rounds_per_half": c.rounds_per_half,
        "buy_ticks": c.buy_ticks, "round_ticks": c.round_ticks, "round_end_ticks": r.round_end_ticks,
        "fuse_ticks": c.fuse_ticks, "plant_ticks": c.plant_ticks, "defuse_ticks": c.defuse_ticks,
        "defuse_kit_ticks": c.defuse_kit_ticks, "unit_radius": c.unit_radius, "walk_speed": c.walk_speed,
        "run_speed": c.run_speed, "turn_rate": c.turn_rate, "fov_half": c.fov_half,
        "vision_range": c.vision_range, "sense_radius": c.sense_radius, "max_hp": c.max_hp,
        "step_sound_radius": c.step_sound_radius, "shot_sound_radius": c.shot_sound_radius,
        "pickup_reach": c.pickup_reach, "interact_reach": c.interact_reach, "max_ticks": c.max_ticks,
        "blast_radius": c.blast_radius, "buy_mem_bytes": c.buy_mem_bytes,
        "team_mem_bytes": c.team_mem_bytes, "draft_mem_bytes": c.draft_mem_bytes,
    })
}

fn economy(r: &Ruleset) -> Value {
    let (e, f) = (&r.econ, &r.flow);
    json!({
        "start_credits": e.start_credits, "max_credits": e.max_credits, "win_reward": e.win_reward,
        "loss_reward": e.loss_reward, "loss_streak_bonus": e.loss_streak_bonus,
        "loss_streak_cap": e.loss_streak_cap, "plant_reward": e.plant_reward,
        "defuse_reward": f.defuse_reward, "ability_kill_reward": f.ability_kill_reward,
        "refund_pct": f.refund_pct, "overtime_credits": f.overtime_credits,
        "overtime_max_pairs": f.overtime_max_pairs, "default_sidearm": e.default_sidearm,
        "armor_absorb_pct": e.armor_absorb_pct,
    })
}

/// The engine's frame JSON with a `loadout` array appended: one entry per unit, in unit order.
pub fn frame(w: &World, frame_json: String) -> String {
    let loadout: Vec<Value> = w
        .units
        .iter()
        .map(|u| {
            json!({
                "id": u.id, "weapons": u.weapons, "ammo": u.ammo, "slot": u.slot as u32,
                "charges": u.abil.charges, "cooldown": u.abil.cooldown, "has_kit": u.has_kit,
                "damage": u.stats.damage, "faulted": u.faulted,
            })
        })
        .collect();
    let Some(open) = frame_json.strip_suffix('}') else {
        return frame_json;
    };
    format!("{open},\"loadout\":{}}}", Value::from(loadout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_carry_every_record() {
        let r = Ruleset::builtin();
        let v: Value = serde_json::from_str(&rules(&r)).unwrap();
        assert_eq!(v["weapon_rules"].as_array().unwrap().len(), r.weapons.len());
        assert_eq!(
            v["ability_rules"].as_array().unwrap().len(),
            r.abilities.len()
        );
        assert_eq!(v["kit_rules"].as_array().unwrap().len(), r.kits.len());
        assert_eq!(v["item_rules"].as_array().unwrap().len(), r.items.len());
        assert_eq!(v["weapon_rules"][1]["name"], r.weapon_names[1]);
        // The base export is kept for the drawing code.
        assert_eq!(v["max_hp"], r.core.max_hp);
        assert!(v["weapons"].is_array());
    }

    #[test]
    fn frame_gains_a_loadout_per_unit() {
        let map = ibc_engine::Map::parse(include_str!("../../../maps/crossfire.ibcmap")).unwrap();
        let m = ibc_engine::Match::new(Ruleset::builtin(), map, 1);
        let v: Value = serde_json::from_str(&frame(&m.world, m.frame_json())).unwrap();
        let units = v["units"].as_array().unwrap().len();
        assert_eq!(v["loadout"].as_array().unwrap().len(), units);
        assert!(v["loadout"][0]["weapons"].is_array());
    }
}
