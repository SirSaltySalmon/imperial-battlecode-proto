//! What the player's page is told each tick: the frame to draw, fogged to what the player's team could know,
//! and the player's unit and match records exactly as a bot on that team reads them.
//!
//! The frame keeps the viewer's shape (`Match::frame_json`) so the replay renderer draws it unchanged, but
//! anything the team could not see is removed or blanked here, on the server: the page never receives an
//! enemy it has not seen, an enemy bullet in the fog or the other team's credits.

use super::input::read;
use ibc_abi::*;
use ibc_engine::world::{World, CHARGE_PLANTED};
use ibc_engine::Match;
use serde_json::{json, Value};

/// What `team` knows after the latest tick: the fogged frame, and each of its units and match records as
/// that unit's bot reads them. Kept for every unit so the player can switch units between ticks.
pub struct TeamView {
    frame: Value,
    /// By slot: (`T_SELF` of the unit, `T_MATCH` as it reads it).
    units: Vec<(Value, Value)>,
}

pub fn team_view(game: &Match, team: i32) -> TeamView {
    let first = team as u32 * game.world.team_size as u32;
    let units = (first..first + game.world.team_size as u32)
        .map(|u| {
            let me = read::<UnitInfo>(game, u, T_SELF).unwrap_or_default();
            let info = read::<MatchInfo>(game, u, T_MATCH).unwrap_or_default();
            (unit_json(&me, &game.world, u), match_json(&info))
        })
        .collect();
    TeamView {
        frame: fogged_frame(game, team),
        units,
    }
}

impl TeamView {
    /// The page's view with the unit in `slot` possessed; `me` is null while the player only watches.
    pub fn view(&self, slot: Option<u32>) -> Value {
        let row = |s: u32| self.units.get(s as usize);
        json!({
            "frame": self.frame,
            "me": slot.and_then(row).map(|(me, _)| me),
            "match": row(slot.unwrap_or(0)).map(|(_, m)| m),
        })
    }
}

/// `frame_json` with what `team` cannot know taken out.
pub fn fogged_frame(game: &Match, team: i32) -> Value {
    let w = &game.world;
    let mut frame: Value = serde_json::from_str(&game.frame_json()).unwrap_or(Value::Null);
    let eyes: Vec<usize> = (0..w.units.len())
        .filter(|&i| w.units[i].team == team && w.units[i].alive)
        .collect();
    // The team's shared view (engine `vision.rs`): every unit any teammate sees.
    let seen = w.views[team as usize & 1].units;
    let sees = |x: i32, y: i32| eyes.iter().any(|&i| w.can_see_point(i, (x, y)));
    let attacking = w.side_of(team) == SIDE_ATTACK;

    if let Some(units) = frame["units"].as_array_mut() {
        for (i, u) in units.iter_mut().enumerate() {
            if w.units[i].team != team {
                let sight = if w.units[i].alive {
                    (seen & (1 << i) != 0).then_some(Sight::Seen)
                } else {
                    // Where an enemy died is public only through the kill event of that tick.
                    w.events
                        .iter()
                        .find(|e| e.kind == EVENT_KILL && e.target == i as i32)
                        .map(|e| Sight::DiedAt(e.x, e.y))
                };
                blank_enemy(u, sight);
            }
        }
    }
    let keep = |v: &mut Value, f: &dyn Fn(&Value) -> bool| {
        if let Some(list) = v.as_array_mut() {
            list.retain(|x| f(x));
        }
    };
    let int = |v: &Value, k: &str| v[k].as_i64().unwrap_or(0) as i32;
    keep(&mut frame["projectiles"], &|p| {
        int(p, "team") == team || sees(int(p, "x"), int(p, "y"))
    });
    keep(&mut frame["trails"], &|t| {
        int(t, "team") == team
            || sees(int(t, "x0"), int(t, "y0"))
            || sees(int(t, "x1"), int(t, "y1"))
    });
    keep(&mut frame["effects"], &|e| {
        int(e, "team") == team || sees(int(e, "x"), int(e, "y"))
    });
    keep(&mut frame["drops"], &|d| {
        (int(d, "kind") == DROP_CHARGE && attacking) || sees(int(d, "x"), int(d, "y"))
    });
    keep(&mut frame["debug"], &|d| {
        d[0].as_u64()
            .and_then(|u| w.units.get(u as usize))
            .is_some_and(|u| u.team == team)
    });
    if !knows_charge(w, team, seen, &sees) {
        frame["charge"] = json!({ "state": -1, "x": 0, "y": 0 });
    }
    frame
}

/// What the team knows of an enemy this tick (`None`: nothing beyond whether it is alive).
#[derive(Clone, Copy)]
enum Sight {
    Seen,
    /// Killed this tick, at this point (the public kill event).
    DiedAt(i32, i32),
}

/// An enemy as a bot sees it (`T_ENEMIES`): no ammo, credits, defuse kit, sounds or vision of its own. Out
/// of sight, nothing but that it is alive; dead, only where it died if that is in this tick's kill event.
/// Blanked rows carry `"hidden": true` so the page does not draw them at the origin.
fn blank_enemy(u: &mut Value, sight: Option<Sight>) {
    u["ammo"] = json!(0);
    u["credits"] = json!(0);
    u["seen"] = json!("0");
    u["sounds"] = json!([]);
    let state = u["state"].as_i64().unwrap_or(0) as i32;
    u["state"] = json!(state & !S_HAS_KIT);
    if matches!(sight, Some(Sight::Seen)) {
        return;
    }
    for k in [
        "x", "y", "vx", "vy", "facing", "aim", "hp", "armor", "kit", "channel",
    ] {
        u[k] = json!(0);
    }
    u["weapon"] = json!(-1);
    u["state"] = json!(state & S_ALIVE);
    match sight {
        Some(Sight::DiedAt(x, y)) => {
            u["x"] = json!(x);
            u["y"] = json!(y);
        }
        _ => u["hidden"] = json!(true),
    }
}

/// Attackers always know where the charge is, defenders once it is planted or in sight.
fn knows_charge(w: &World, team: i32, seen: u64, sees: &dyn Fn(i32, i32) -> bool) -> bool {
    if w.side_of(team) == SIDE_ATTACK || w.charge_state == CHARGE_PLANTED {
        return true;
    }
    match w.units.iter().position(|u| u.has_charge) {
        Some(carrier) => w.units[carrier].team == team || seen & (1 << carrier) != 0,
        None => sees(w.charge_pos.0, w.charge_pos.1),
    }
}

fn unit_json(u: &UnitInfo, w: &World, unit: u32) -> Value {
    let full = w.units.get(unit as usize);
    json!({
        "id": u.id, "team": u.team, "side": u.side, "kit": u.kit, "state": u.state, "x": u.x, "y": u.y,
        "facing": u.facing, "hp": u.hp, "armor": u.armor, "weapon": u.weapon, "ammo": u.ammo,
        "channel": u.channel, "credits": u.credits, "charges": u.charges, "cooldown": u.cooldown,
        "recoil": u.recoil, "blind": u.blind,
        // The weapon slots are the unit's own loadout, which its bot knows (it bought them).
        "weapons": full.map(|f| f.weapons.to_vec()).unwrap_or_default(),
        "slot": full.map(|f| f.slot as i32).unwrap_or(0),
    })
}

fn match_json(m: &MatchInfo) -> Value {
    json!({
        "tick": m.tick, "round": m.round, "phase": m.phase, "phase_ticks_left": m.phase_ticks_left,
        "my_team": m.my_team, "my_side": m.my_side, "score_mine": m.score_mine, "score_theirs": m.score_theirs,
        "allies_alive": m.allies_alive, "enemies_alive": m.enemies_alive, "charge_state": m.charge_state,
        "fuse_left": m.fuse_left, "rounds_to_win": m.rounds_to_win, "unit_slot": m.unit_slot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ibc_engine::{Map, Ruleset};

    const MAP: &str = include_str!("../../../../maps/crossfire.ibcmap");

    #[test]
    fn hides_unseen_enemies_and_enemy_secrets() {
        let mut m = Match::new(Ruleset::builtin(), Map::parse(MAP).unwrap(), 3);
        while m.world.phase != PHASE_ACTION {
            m.step();
        }
        let team_size = m.world.team_size as usize;
        let tv = team_view(&m, 0);
        let v = tv.view(Some(0));
        assert_eq!(
            tv.view(None)["me"],
            Value::Null,
            "watching: no unit of one's own"
        );
        assert_eq!(tv.view(Some(1))["me"]["id"], json!(1));
        let units = v["frame"]["units"].as_array().unwrap();
        assert_eq!(
            units.len(),
            2 * team_size,
            "every unit keeps its row so indices stay unit ids"
        );
        for (i, u) in units.iter().enumerate() {
            let w = &m.world.units[i];
            if w.team == 0 {
                assert_eq!(u["x"], json!(w.pos.0), "own team in full");
                assert_eq!(u["credits"], json!(w.credits));
            } else {
                // At round start the teams are at opposite spawns and cannot see each other.
                assert_eq!(
                    (u["x"].clone(), u["credits"].clone(), u["seen"].clone()),
                    (json!(0), json!(0), json!("0"))
                );
                assert_eq!(
                    (u["alive"].clone(), u["hidden"].clone()),
                    (json!(true), json!(true))
                );
            }
        }
        // A seen enemy keeps its position but not its defuse kit; a dead one only shows where it died in the
        // tick of its kill event.
        let mut u =
            serde_json::from_str::<Value>(&m.frame_json()).unwrap()["units"][team_size].clone();
        u["state"] = json!(S_ALIVE | S_HAS_KIT);
        blank_enemy(&mut u, Some(Sight::Seen));
        assert_eq!(
            (u["state"].clone(), u["x"].clone()),
            (json!(S_ALIVE), json!(m.world.units[team_size].pos.0))
        );
        u["alive"] = json!(false);
        blank_enemy(&mut u, Some(Sight::DiedAt(7, 9)));
        assert_eq!(
            (u["x"].clone(), u["y"].clone(), u["hidden"].clone()),
            (json!(7), json!(9), Value::Null)
        );
        blank_enemy(&mut u, None);
        assert_eq!(
            (u["x"].clone(), u["hidden"].clone()),
            (json!(0), json!(true))
        );
        assert_eq!(v["me"]["id"], json!(0));
        assert_eq!(v["me"]["credits"], json!(m.world.units[0].credits));
        assert_eq!(v["match"]["phase"], json!(PHASE_ACTION));
    }
}
