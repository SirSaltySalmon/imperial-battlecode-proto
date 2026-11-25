//! The ruleset: every tunable number in the game, loaded from `rules/*.toml`. Balance patches are data changes.
//! Bots read the same values at runtime through T_RULES_CORE / T_WEAPON / T_ABILITY / T_ITEM / T_KIT.

use ibc_abi::{AbilityRules, CoreRules, ItemRules, KitRules, WeaponRules};
use toml::Table;

/// Economy numbers that bots do not need as a table (they see prices through items and weapons).
#[derive(Clone, Copy, Debug, Default)]
pub struct EconRules {
    pub start_credits: i32,
    pub max_credits: i32,
    pub win_reward: i32,
    pub loss_reward: i32,
    pub loss_streak_bonus: i32,
    pub loss_streak_cap: i32,
    pub plant_reward: i32,
    pub default_sidearm: i32,
    pub armor_absorb_pct: i32,
}

#[derive(Clone, Debug)]
pub struct Ruleset {
    pub core: CoreRules,
    pub round_end_ticks: i32,
    pub weapons: Vec<WeaponRules>,
    pub weapon_names: Vec<String>,
    pub abilities: Vec<AbilityRules>,
    pub ability_names: Vec<String>,
    pub kits: Vec<KitRules>,
    pub kit_names: Vec<String>,
    pub items: Vec<ItemRules>,
    pub item_names: Vec<String>,
    pub econ: EconRules,
    /// CORE CHANGE (unit/economy): income and overtime numbers owned and parsed by the economy module.
    pub flow: crate::economy::FlowRules,
}

fn gi(t: &Table, k: &str, d: i32) -> i32 {
    t.get(k).and_then(|v| v.as_integer()).map(|v| v as i32).unwrap_or(d)
}

fn gs(t: &Table, k: &str) -> String {
    t.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

fn list<'a>(t: &'a Table, k: &str) -> Vec<&'a Table> {
    t.get(k).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_table()).collect()).unwrap_or_default()
}

impl Ruleset {
    /// The ruleset compiled into this engine build.
    pub fn builtin() -> Ruleset {
        Ruleset::from_toml(
            include_str!("../../../rules/core.toml"),
            include_str!("../../../rules/weapons.toml"),
            include_str!("../../../rules/abilities.toml"),
            include_str!("../../../rules/economy.toml"),
        )
        .expect("built-in ruleset must parse")
    }

    pub fn from_toml(core: &str, weapons: &str, abilities: &str, economy: &str) -> Result<Ruleset, String> {
        let p = |s: &str, what: &str| s.parse::<Table>().map_err(|e| format!("{what}: {e}"));
        let (c, w, a, e) = (p(core, "core")?, p(weapons, "weapons")?, p(abilities, "abilities")?, p(economy, "economy")?);

        let mut weapon_names = Vec::new();
        let weapons: Vec<WeaponRules> = list(&w, "weapon")
            .iter()
            .enumerate()
            .map(|(i, t)| {
                weapon_names.push(gs(t, "name"));
                WeaponRules {
                    id: i as i32,
                    price: gi(t, "price", 0),
                    damage: gi(t, "damage", 10),
                    pellets: gi(t, "pellets", 1).max(1),
                    spread: gi(t, "spread", 0),
                    move_spread: gi(t, "move_spread", 0),
                    recoil_spread: gi(t, "recoil_spread", 0),
                    recoil_per_shot: gi(t, "recoil_per_shot", 0),
                    recoil_recovery: gi(t, "recoil_recovery", 1),
                    windup: gi(t, "windup", 1).max(0),
                    cooldown: gi(t, "cooldown", 0),
                    magazine: gi(t, "magazine", 10).max(1),
                    reload_ticks: gi(t, "reload_ticks", 3).max(1),
                    bullet_speed: gi(t, "bullet_speed", 8192).max(256),
                    range: gi(t, "range", 20480),
                    falloff_start: gi(t, "falloff_start", 20480),
                    falloff_min_pct: gi(t, "falloff_min_pct", 100),
                    move_pct: gi(t, "move_pct", 100),
                    kill_reward: gi(t, "kill_reward", 200),
                    slot: gi(t, "slot", 0).clamp(0, 1),
                }
            })
            .collect();
        if weapons.is_empty() {
            return Err("weapons: need at least one [[weapon]]".into());
        }

        let mut ability_names = Vec::new();
        let abilities: Vec<AbilityRules> = list(&a, "ability")
            .iter()
            .enumerate()
            .map(|(i, t)| {
                ability_names.push(gs(t, "name"));
                AbilityRules {
                    id: i as i32,
                    effect: gi(t, "effect", 0),
                    price: gi(t, "price", 0),
                    max_charges: gi(t, "max_charges", 1),
                    cooldown: gi(t, "cooldown", 0),
                    range: gi(t, "range", 0),
                    radius: gi(t, "radius", 0),
                    duration: gi(t, "duration", 0),
                    power: gi(t, "power", 0),
                    delay: gi(t, "delay", 0),
                }
            })
            .collect();

        let mut kit_names = Vec::new();
        let kits: Vec<KitRules> = list(&a, "kit")
            .iter()
            .enumerate()
            .map(|(i, t)| {
                kit_names.push(gs(t, "name"));
                let ab: Vec<i32> = t
                    .get("abilities")
                    .and_then(|v| v.as_array())
                    .map(|v| v.iter().filter_map(|x| x.as_integer()).map(|x| x as i32).collect())
                    .unwrap_or_default();
                KitRules {
                    id: i as i32,
                    abilities: [ab.first().copied().unwrap_or(-1), ab.get(1).copied().unwrap_or(-1)],
                    hp: gi(t, "hp", 100),
                    speed_pct: gi(t, "speed_pct", 100),
                }
            })
            .collect();
        if kits.is_empty() {
            return Err("abilities: need at least one [[kit]]".into());
        }

        let mut item_names = Vec::new();
        let items: Vec<ItemRules> = list(&e, "item")
            .iter()
            .enumerate()
            .map(|(i, t)| {
                item_names.push(gs(t, "name"));
                ItemRules { id: i as i32, kind: gi(t, "kind", 0), price: gi(t, "price", 0), value: gi(t, "value", 0) }
            })
            .collect();

        let core = CoreRules {
            team_size: gi(&c, "team_size", 5),
            rounds_to_win: gi(&c, "rounds_to_win", 7),
            rounds_per_half: gi(&c, "rounds_per_half", 6),
            buy_ticks: gi(&c, "buy_ticks", 3).max(1),
            round_ticks: gi(&c, "round_ticks", 120),
            fuse_ticks: gi(&c, "fuse_ticks", 40),
            plant_ticks: gi(&c, "plant_ticks", 4),
            defuse_ticks: gi(&c, "defuse_ticks", 7),
            defuse_kit_ticks: gi(&c, "defuse_kit_ticks", 4),
            unit_radius: gi(&c, "unit_radius", 300),
            walk_speed: gi(&c, "walk_speed", 350),
            run_speed: gi(&c, "run_speed", 700),
            turn_rate: gi(&c, "turn_rate", 20000),
            fov_half: gi(&c, "fov_half", 9100),
            vision_range: gi(&c, "vision_range", 24576),
            sense_radius: gi(&c, "sense_radius", 1536),
            max_hp: gi(&c, "max_hp", 100),
            step_sound_radius: gi(&c, "step_sound_radius", 8192),
            shot_sound_radius: gi(&c, "shot_sound_radius", 30720),
            substeps: gi(&c, "substeps", 8).clamp(1, 64),
            fuel_per_tick: gi(&c, "fuel_per_tick", 100_000),
            fuel_first_tick_mult: gi(&c, "fuel_first_tick_mult", 10),
            fuel_buy_mult: gi(&c, "fuel_buy_mult", 4),
            team_mem_bytes: gi(&c, "team_mem_bytes", 64),
            draft_mem_bytes: gi(&c, "draft_mem_bytes", 1024),
            pickup_reach: gi(&c, "pickup_reach", 900),
            interact_reach: gi(&c, "interact_reach", 1200),
            weapons: weapons.len() as i32,
            abilities: abilities.len() as i32,
            items: items.len() as i32,
            kits: kits.len() as i32,
            max_ticks: gi(&c, "max_ticks", 3000),
        };
        let econ = EconRules {
            start_credits: gi(&e, "start_credits", 800),
            max_credits: gi(&e, "max_credits", 9000),
            win_reward: gi(&e, "win_reward", 3000),
            loss_reward: gi(&e, "loss_reward", 1900),
            loss_streak_bonus: gi(&e, "loss_streak_bonus", 500),
            loss_streak_cap: gi(&e, "loss_streak_cap", 2),
            plant_reward: gi(&e, "plant_reward", 300),
            default_sidearm: gi(&e, "default_sidearm", 0).clamp(0, weapons.len() as i32 - 1),
            armor_absorb_pct: gi(&e, "armor_absorb_pct", 50),
        };
        Ok(Ruleset {
            core,
            round_end_ticks: gi(&c, "round_end_ticks", 2).max(1),
            weapons,
            weapon_names,
            abilities,
            ability_names,
            kits,
            kit_names,
            items,
            item_names,
            econ,
            flow: crate::economy::FlowRules::from_table(&e),
        })
    }

    /// Hash of every number that affects simulation. Part of the patch identity.
    pub fn hash(&self) -> u64 {
        let mut h = crate::Fnv::new();
        h.bytes(ibc_abi::bytes_of(&self.core));
        h.i32(self.round_end_ticks);
        for w in &self.weapons {
            h.bytes(ibc_abi::bytes_of(w));
        }
        for a in &self.abilities {
            h.bytes(ibc_abi::bytes_of(a));
        }
        for k in &self.kits {
            h.bytes(ibc_abi::bytes_of(k));
        }
        for i in &self.items {
            h.bytes(ibc_abi::bytes_of(i));
        }
        let e = &self.econ;
        for v in [
            e.start_credits,
            e.max_credits,
            e.win_reward,
            e.loss_reward,
            e.loss_streak_bonus,
            e.loss_streak_cap,
            e.plant_reward,
            e.default_sidearm,
            e.armor_absorb_pct,
        ] {
            h.i32(v);
        }
        self.flow.hash(&mut h);
        h.finish()
    }
}
