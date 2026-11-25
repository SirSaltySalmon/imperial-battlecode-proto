//! One frame of a scripted fight, compact enough to inline in the lab page.
//!
//! Unit row: x, y, facing, team, alive, hp, armor, weapon, ammo, recoil, winding, channel, cooldown, aim,
//! reload, shots, weapon0, ammo0, weapon1, ammo1, slot.
//! Bullet row: x, y, weapon, vx, vy, owner.
//! Drop row: x, y, item. Hit row: actor, target, amount.

use ibc_engine::abi::*;
use ibc_engine::Match;
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Lab {
    pub tile: i32,
    pub radius: i32,
    pub fov_half: i32,
    pub max_hp: i32,
    pub run_speed: i32,
    pub heavy_armor: i32,
    pub vision_range: i32,
    pub sense_radius: i32,
    pub ruleset: String,
    pub weapons: Vec<WeaponOut>,
    pub maps: Vec<MapOut>,
    pub scenes: Vec<Scene>,
    pub checks: Vec<FanCheck>,
    pub damage_checks: Vec<DamageCheck>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeaponOut {
    pub name: String,
    pub id: i32,
    pub slot: i32,
    pub price: i32,
    pub damage: i32,
    pub pellets: i32,
    pub spread: i32,
    pub move_spread: i32,
    pub recoil_spread: i32,
    pub recoil_per_shot: i32,
    pub recoil_recovery: i32,
    pub windup: i32,
    pub cooldown: i32,
    pub magazine: i32,
    pub reload_ticks: i32,
    pub bullet_speed: i32,
    pub range: i32,
    pub falloff_start: i32,
    pub falloff_min_pct: i32,
    pub move_pct: i32,
    pub kill_reward: i32,
    pub top_speed: i32,
    /// Heat, sideways shift, and fan step at each release while holding the trigger, standing still.
    pub hold: Vec<[i32; 3]>,
}

#[derive(Serialize)]
pub struct MapOut {
    pub id: String,
    pub w: i32,
    pub h: i32,
    pub tiles: String,
}

#[derive(Serialize)]
pub struct TtkKey {
    pub weapon: String,
    pub tiles: i32,
    pub running: bool,
    pub armor: i32,
}

#[derive(Serialize)]
pub struct Scene {
    pub id: String,
    pub group: String,
    pub title: String,
    pub note: String,
    pub outcome: String,
    pub map: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttk: Option<TtkKey>,
    pub frames: Vec<Frame>,
}

#[derive(Serialize)]
pub struct Frame {
    pub u: Vec<Vec<i32>>,
    pub b: Vec<Vec<i32>>,
    pub d: Vec<Vec<i32>>,
    pub hit: Vec<Vec<i32>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FanCheck {
    pub weapon: i32,
    pub facing: i32,
    pub aim: i32,
    pub speed: i32,
    pub recoil: i32,
    pub shots: i32,
    pub centre: i32,
    pub step: i32,
}

#[derive(Serialize)]
pub struct DamageCheck {
    pub weapon: i32,
    pub travelled: i32,
    pub damage: i32,
}

pub struct Meta {
    pub id: String,
    pub group: String,
    pub title: String,
    pub note: String,
    pub outcome: String,
    pub ttk: Option<TtkKey>,
}

pub fn snap(m: &Match) -> Frame {
    let mut u = Vec::new();
    for unit in &m.world.units {
        u.push(vec![
            unit.pos.0,
            unit.pos.1,
            unit.facing,
            unit.team,
            i32::from(unit.alive),
            unit.hp,
            unit.armor,
            unit.weapon(),
            unit.ammo[unit.slot],
            unit.combat.recoil,
            i32::from(unit.combat.winding),
            unit.combat.channel(),
            unit.combat.cooldown,
            unit.combat.aim & 0xFFFF,
            unit.combat.reload,
            unit.combat.shots,
            unit.weapons[0],
            unit.ammo[0],
            unit.weapons[1],
            unit.ammo[1],
            unit.slot as i32,
        ]);
    }
    let b = m
        .world
        .projectiles
        .iter()
        .filter(|p| p.kind == 0)
        .map(|p| vec![p.pos.0, p.pos.1, p.weapon, p.vel.0, p.vel.1, p.owner as i32])
        .collect();
    let d = m
        .world
        .drops
        .iter()
        .filter(|drop| drop.kind == DROP_WEAPON)
        .map(|drop| vec![drop.x, drop.y, drop.item])
        .collect();
    let hit = m
        .world
        .events
        .iter()
        .filter(|e| e.kind == EVENT_DAMAGE)
        .map(|e| vec![e.actor, e.target, e.value])
        .collect();
    Frame { u, b, d, hit }
}

pub struct Catalog {
    pub maps: Vec<MapOut>,
    pub scenes: Vec<Scene>,
    pub hold: Vec<Vec<[i32; 3]>>,
}

impl Catalog {
    pub fn new() -> Catalog {
        Catalog {
            maps: Vec::new(),
            scenes: Vec::new(),
            hold: Vec::new(),
        }
    }

    pub fn intern(&mut self, m: &Match) -> String {
        let map = &m.world.map;
        let tiles: String = map
            .tiles
            .iter()
            .map(|t| match *t {
                TILE_WALL => '#',
                TILE_LOW => 'o',
                _ => '.',
            })
            .collect();
        let id = format!("{}x{}-{:08x}", map.width, map.height, fnv(&tiles) as u32);
        if !self.maps.iter().any(|got| got.id == id) {
            self.maps.push(MapOut {
                id: id.clone(),
                w: map.width,
                h: map.height,
                tiles,
            });
        }
        id
    }

    pub fn add(&mut self, m: &Match, meta: Meta, frames: Vec<Frame>) {
        let map = self.intern(m);
        self.scenes.push(Scene {
            id: meta.id,
            group: meta.group,
            title: meta.title,
            note: meta.note,
            outcome: meta.outcome,
            map,
            ttk: meta.ttk,
            frames,
        });
    }
}

fn fnv(tiles: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in tiles.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x100000001b3);
    }
    h
}
