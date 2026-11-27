//! Per-weapon numbers for `balance`: what a weapon is worth to the units that carry it.
//!
//! Kill share is not strength: it is carriers times kills per carrier. The **kill index** of a weapon is its
//! carriers' kills per carrier-round over all kills per unit-round, a carrier-round being a unit holding the
//! weapon when the action phase starts. Every enemy kill is credited to the weapon its killer carried then,
//! whatever finished the kill (sidearm and abilities included), so the indices average to 1 over the
//! carrier-rounds. Splits compare like with like: by side, by bot (the index inside one bot's games: the
//! bot's kills per carrier-round over the bot's kills per unit-round) and by the victim's armour at action
//! start. Kill distance is the other attribution: kills made *with* the weapon. A unit holding no weapon
//! at all at action start is left out of both sides of the index.
//!
//! Only details from `DETAIL_VERSION` 1 on carry the attribution; older games are left out of these
//! metrics, and when none has it the section is marked unavailable.

use crate::balance::{band_of, weapon_name, Metrics, BANDS};
use crate::metrics::{Detail, KillRec, DETAIL_VERSION};
use crate::model::GameResult;
use ibc_abi::{TEAM_A, TEAM_B, TILE};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const SIDES: [&str; 2] = ["attack", "defend"];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PerWeapon {
    /// Games whose details carry per-weapon attribution. 0: the per-weapon metrics are unavailable
    /// (details recorded by an older `ibc-bench`).
    pub games: u64,
    /// Weapons carried or killed with, by weapon id.
    pub weapons: Vec<String>,
    /// Victim armour classes seen, "none" first.
    pub armor_classes: Vec<String>,
    /// Carrier-rounds per weapon over all those games.
    pub carrier_rounds: BTreeMap<String, u64>,
}

fn attributed(g: &GameResult) -> Option<&Detail> {
    g.detail.as_ref().filter(|d| d.version >= DETAIL_VERSION)
}

fn id_of(d: &Detail, name: &str) -> Option<i32> {
    (0..d.weapon_names.len() as i32).find(|&i| weapon_name(&d.weapon_names, i) == name)
}

/// Which carrier-rounds and kills a cell counts: a side (0 attack), a bot label, a victim armour class.
#[derive(Clone, Copy, Default)]
struct Cell<'a> {
    side: Option<usize>,
    bot: Option<&'a str>,
    armor: Option<&'a str>,
}

impl Cell<'_> {
    fn takes(&self, g: &GameResult, side: usize, team: i32) -> bool {
        let label = if team == TEAM_A { &g.a } else { &g.b };
        self.side.is_none_or(|s| s == side) && self.bot.is_none_or(|b| b == label)
    }
}

/// One game's sums for one weapon and cell: `[kills by its carriers, its carrier-rounds, all kills,
/// unit-rounds]` (the kill index's clusters) and the kill reward its carriers earned.
fn tally(g: &GameResult, weapon: &str, cell: Cell) -> ([f64; 4], f64) {
    let Some(d) = attributed(g) else {
        return ([0.0; 4], 0.0);
    };
    let id = id_of(d, weapon);
    let (mut sums, mut reward) = ([0.0; 4], 0.0);
    let attack_team = |round: i32| {
        d.rounds
            .iter()
            .find(|r| r.round == round)
            .map(|r| r.attack_team)
    };
    for r in &d.rounds {
        for (side, units) in r.loadout.iter().enumerate() {
            let team = match (side, r.attack_team) {
                (0, t) => t,
                (_, TEAM_A) => TEAM_B,
                _ => TEAM_A,
            };
            if cell.takes(g, side, team) {
                // A unit with no weapon at all carries nothing to credit: it is not a unit-round.
                sums[3] += units.iter().filter(|&&w| w >= 0).count() as f64;
                sums[1] += units.iter().filter(|&&w| Some(w) == id).count() as f64;
            }
        }
    }
    for k in d.kills.iter().filter(|k| k.enemy() && k.carried >= 0) {
        // A kill in a round the details did not close (none, in a finished game) has no unit-rounds.
        let Some(atk) = attack_team(k.round) else {
            continue;
        };
        let side = (k.team != atk) as usize;
        if !cell.takes(g, side, k.team)
            || cell
                .armor
                .is_some_and(|a| d.armor_class(k.victim_armor) != a)
        {
            continue;
        }
        sums[2] += 1.0;
        if Some(k.carried) == id {
            sums[0] += 1.0;
            reward += d.reward(k.weapon) as f64;
        }
    }
    (sums, reward)
}

/// Enemy kills made with `weapon` that have a distance.
fn kills_with<'a>(g: &'a GameResult, weapon: &'a str) -> impl Iterator<Item = &'a KillRec> {
    attributed(g).into_iter().flat_map(move |d| {
        let id = id_of(d, weapon);
        d.kills
            .iter()
            .filter(move |k| k.enemy() && k.dist >= 0 && Some(k.weapon) == id)
    })
}

fn damage_by(g: &GameResult, weapon: &str) -> f64 {
    attributed(g).map_or(0.0, |d| {
        id_of(d, weapon)
            .and_then(|i| d.damage_by_carried.get(i as usize))
            .map_or(0.0, |s| (s[0] + s[1]) as f64)
    })
}

fn index_metrics(m: &mut Metrics, w: &str, bots: &[String], armor: &[String]) {
    let key = |rest: &str| format!("weapon.{w}.{rest}");
    m.add_index(&key("kill_index"), |g| tally(g, w, Cell::default()).0);
    for (side, name) in SIDES.into_iter().enumerate() {
        let cell = Cell {
            side: Some(side),
            ..Default::default()
        };
        m.add_index(&key(&format!("kill_index.side.{name}")), |g| {
            tally(g, w, cell).0
        });
    }
    for bot in bots {
        let cell = Cell {
            bot: Some(bot),
            ..Default::default()
        };
        m.add_index(&key(&format!("kill_index.bot.{bot}")), |g| {
            tally(g, w, cell).0
        });
    }
    for class in armor {
        let cell = Cell {
            armor: Some(class),
            ..Default::default()
        };
        m.add_index(&key(&format!("kill_index.victim_armor.{class}")), |g| {
            tally(g, w, cell).0
        });
    }
}

fn per_carrier_metrics(m: &mut Metrics, w: &str) {
    let key = |rest: &str| format!("weapon.{w}.{rest}");
    m.add(&key("kills_per_carrier_round"), false, |g| {
        let (s, _) = tally(g, w, Cell::default());
        (s[0], s[1])
    });
    m.add(&key("damage_per_carrier_round"), false, |g| {
        (damage_by(g, w), tally(g, w, Cell::default()).0[1])
    });
    m.add(&key("kill_reward_per_carrier_round"), false, |g| {
        let (s, reward) = tally(g, w, Cell::default());
        (reward, s[1])
    });
}

fn distance_metrics(m: &mut Metrics, w: &str) {
    let key = |rest: &str| format!("weapon.{w}.{rest}");
    m.add(&key("kill_distance_tiles.mean"), false, |g| {
        let tiles = kills_with(g, w).map(|k| k.dist as f64 / TILE as f64);
        (tiles.sum(), kills_with(g, w).count() as f64)
    });
    for (band, _) in BANDS {
        m.add(&key(&format!("kills.distance.{band}.share")), true, |g| {
            (
                kills_with(g, w).filter(|k| band_of(k.dist) == band).count() as f64,
                kills_with(g, w).count() as f64,
            )
        });
    }
}

/// Add every per-weapon metric to `m` and describe what they cover.
pub(crate) fn weapon_kill_metrics(m: &mut Metrics, bots: &[String]) -> PerWeapon {
    let games: Vec<&Detail> = m.games.iter().filter_map(|g| attributed(g)).collect();
    let mut ids: BTreeSet<(i32, String)> = BTreeSet::new();
    let mut carrier_rounds: BTreeMap<String, u64> = BTreeMap::new();
    let mut armor: BTreeSet<(i32, String)> = BTreeSet::new();
    for d in &games {
        let carried = d.rounds.iter().flat_map(|r| r.loadout.iter().flatten());
        for &w in carried.filter(|&&w| w >= 0) {
            ids.insert((w, weapon_name(&d.weapon_names, w)));
            *carrier_rounds
                .entry(weapon_name(&d.weapon_names, w))
                .or_default() += 1;
        }
        for k in d.kills.iter().filter(|k| k.enemy()) {
            ids.insert((k.weapon, weapon_name(&d.weapon_names, k.weapon)));
            armor.insert((k.victim_armor.max(0), d.armor_class(k.victim_armor)));
        }
    }
    // Ordered by armour points ("none" first); one name may cover several values (`armor_<v>` does not).
    let mut armor_classes: Vec<String> = Vec::new();
    for (_, a) in armor {
        if !armor_classes.contains(&a) {
            armor_classes.push(a);
        }
    }
    // Abilities (-1) are not a weapon.
    let mut weapons: Vec<String> = Vec::new();
    for (id, name) in ids {
        if id >= 0 && !weapons.contains(&name) {
            weapons.push(name);
        }
    }
    for w in &weapons {
        index_metrics(m, w, bots, &armor_classes);
        per_carrier_metrics(m, w);
        distance_metrics(m, w);
    }
    PerWeapon {
        games: games.len() as u64,
        weapons,
        armor_classes,
        carrier_rounds,
    }
}
