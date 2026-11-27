//! `ibc-bench dump` and `ibc-bench why`: look inside one replay.
//!
//! `dump` writes a compact per-tick JSONL and a per-round CSV for ad-hoc analysis. `why` prints a readable
//! timeline of one unit in one round: what its team saw and heard (vision and sound are shared), what it asked
//! for (with the engine's answer to every action) and what happened to it.

use crate::balance::reason_name;
use crate::metrics::Collector;
use ibc_abi::*;
use ibc_engine::{fixed, Input, Match};
use ibc_replay::{Event, Replay};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::path::Path;

pub fn load_replay(path: &Path) -> Result<Replay, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Replay::decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// One compact row per tick, built from the engine's `frame_json` so it follows that format's fields:
/// `u` = `[id, alive, x, y, facing, hp, armor, weapon, ammo, credits, state, seen]` per unit,
/// `ev` = `[kind, actor, target, x, y, value]` per event, `p` = `[owner, weapon, x, y]` per projectile.
fn tick_row(frame: &Value) -> Value {
    let list = |v: &Value| v.as_array().cloned().unwrap_or_default();
    let units: Vec<Value> = list(&frame["units"])
        .iter()
        .map(|u| {
            json!([
                u["id"],
                u["alive"].as_bool().unwrap_or(false) as u8,
                u["x"],
                u["y"],
                u["facing"],
                u["hp"],
                u["armor"],
                u["weapon"],
                u["ammo"],
                u["credits"],
                u["state"],
                u["seen"]
            ])
        })
        .collect();
    let events: Vec<Value> = list(&frame["events"])
        .iter()
        .map(|e| {
            json!([
                e["kind"],
                e["actor"],
                e["target"],
                e["x"],
                e["y"],
                e["value"]
            ])
        })
        .collect();
    let shots: Vec<Value> = list(&frame["projectiles"])
        .iter()
        .map(|p| json!([p["owner"], p["weapon"], p["x"], p["y"]]))
        .collect();
    json!({
        "t": frame["tick"], "r": frame["round"], "ph": frame["phase"], "left": frame["phase_left"],
        "score": frame["score"], "atk": frame["attackers"],
        "charge": [frame["charge"]["state"], frame["charge"]["x"], frame["charge"]["y"]],
        "u": units, "p": shots, "ev": events,
    })
}

pub struct Dump {
    pub ticks_jsonl: String,
    pub rounds_csv: String,
}

pub fn dump(replay: &Replay) -> Result<Dump, String> {
    let mut ticks = String::new();
    let mut collector = Collector::new();
    let mut bad_frame = None;
    let m = replay.simulate(|m| {
        collector.observe(m);
        match serde_json::from_str::<Value>(&m.frame_json()) {
            Ok(frame) => {
                let _ = writeln!(ticks, "{}", tick_row(&frame));
            }
            Err(e) => bad_frame = Some(e.to_string()),
        }
    })?;
    if let Some(e) = bad_frame {
        return Err(format!("frame_json is not JSON: {e}"));
    }
    let detail = collector.finish();
    let mut csv = String::from(
        "round,attackers,winner,reason,ticks,planted,first_contact,first_kill,shots,kills,damage,moved_tiles,\
         atk_credits,def_credits,atk_spent,def_spent\n",
    );
    let opt = |v: Option<i32>| v.map(|x| x.to_string()).unwrap_or_default();
    let planted = &m.stats().planted;
    for (i, (log, d)) in m.world.rounds.iter().zip(&detail.rounds).enumerate() {
        let _ = writeln!(
            csv,
            "{},{},{},{},{},{},{},{},{},{},{},{:.1},{},{},{},{}",
            log.round,
            log.attackers,
            log.winner,
            reason_name(log.reason),
            log.ticks,
            planted.get(i).copied().unwrap_or(false) as u8,
            opt(d.first_contact),
            opt(d.first_kill),
            d.shots,
            d.kills,
            d.damage,
            d.moved as f64 / TILE as f64,
            d.credits_start[0],
            d.credits_start[1],
            d.spent[0],
            d.spent[1]
        );
    }
    Ok(Dump {
        ticks_jsonl: ticks,
        rounds_csv: csv,
    })
}

fn deg(angle: i32) -> i32 {
    ((angle & 0xFFFF) as i64 * 360 / FULL_TURN as i64) as i32
}

fn tiles(p: (i32, i32)) -> String {
    format!(
        "({:.1}, {:.1})",
        p.0 as f64 / TILE as f64,
        p.1 as f64 / TILE as f64
    )
}

fn phase_name(phase: i32) -> &'static str {
    match phase {
        PHASE_DRAFT => "draft",
        PHASE_BUY => "buy",
        PHASE_ACTION => "action",
        PHASE_PLANTED => "planted",
        PHASE_ROUND_END => "round end",
        PHASE_MATCH_END => "match end",
        _ => "?",
    }
}

fn error_name(code: i32) -> String {
    match code {
        OK => "ok".into(),
        ERR_BAD_ARG => "ERR_BAD_ARG".into(),
        ERR_UNSUPPORTED => "ERR_UNSUPPORTED".into(),
        ERR_NOT_ALLOWED => "ERR_NOT_ALLOWED".into(),
        ERR_NOT_READY => "ERR_NOT_READY".into(),
        ERR_OUT_OF_RANGE => "ERR_OUT_OF_RANGE".into(),
        ERR_INSUFFICIENT_FUNDS => "ERR_INSUFFICIENT_FUNDS".into(),
        other => format!("error {other}"),
    }
}

fn sound_name(kind: i32) -> &'static str {
    match kind {
        SOUND_STEP => "steps",
        SOUND_SHOT => "shot",
        SOUND_PLANT => "plant",
        SOUND_DEFUSE => "defuse",
        SOUND_ABILITY => "ability",
        SOUND_DECOY => "decoy",
        _ => "sound",
    }
}

/// `(text, is a movement or facing order)`.
fn describe_act(m: &Match, kind: u32, a: i32, b: i32, c: i32) -> (String, bool) {
    let rules = &m.world.rules;
    let named = |names: &[String], id: i32| {
        names
            .get(id.max(0) as usize)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    };
    match kind {
        A_MOVE => (format!("MOVE {}deg speed {b}", deg(a)), true),
        A_FACE => (format!("FACE {}deg", deg(a)), true),
        A_FIRE => (format!("FIRE {}deg", deg(a)), false),
        A_RELOAD => ("RELOAD".into(), false),
        A_PLANT => ("PLANT".into(), false),
        A_DEFUSE => ("DEFUSE".into(), false),
        A_ABILITY => (format!("ABILITY slot {a} at {}", tiles((b, c))), false),
        A_BUY => (format!("BUY {}", named(&rules.item_names, a)), false),
        A_SWAP => (format!("SWAP to slot {a}"), false),
        A_DROP => (format!("DROP {a}"), false),
        A_PICKUP => ("PICKUP".into(), false),
        A_DRAFT => (format!("DRAFT {}", named(&rules.kit_names, a)), false),
        A_CANCEL => ("CANCEL".into(), false),
        A_BUY_FOR => (
            format!("BUY {} for slot {b}", named(&rules.item_names, a)),
            false,
        ),
        A_DRAFT_FOR => (
            format!("DRAFT {} for slot {b}", named(&rules.kit_names, a)),
            false,
        ),
        other => (format!("action {other}({a}, {b}, {c})"), false),
    }
}

/// What the unit could perceive at the start of a tick: the enemies its team sees and the sounds its team heard.
fn perception(m: &Match, unit: usize) -> (Vec<u32>, String, String) {
    let w = &m.world;
    let me = &w.units[unit];
    let view = &w.views[me.team as usize];
    let mut ids = Vec::new();
    let mut sees = Vec::new();
    for o in w
        .units
        .iter()
        .filter(|o| o.alive && o.team != me.team && view.units & (1u64 << o.id) != 0)
    {
        ids.push(o.id);
        let mut flags = String::new();
        if o.combat.winding {
            flags.push_str(" winding up");
        }
        if o.has_charge {
            flags.push_str(" carrying the charge");
        }
        sees.push(format!(
            "#{} at {} ({:.1} tiles away, hp {}{flags})",
            o.id,
            tiles(o.pos),
            fixed::dist(me.pos, o.pos) as f64 / TILE as f64,
            o.hp
        ));
    }
    let hears: Vec<String> = w.team_sounds[me.team as usize]
        .iter()
        .map(|s| format!("{} at {}", sound_name(s.kind), tiles((s.x, s.y))))
        .collect();
    (ids, sees.join(", "), hears.join(", "))
}

/// What the step did to or through the unit, from the public events of the tick.
fn consequences(m: &Match, unit: usize) -> Vec<String> {
    let w = &m.world;
    let me = unit as i32;
    let weapon = |id: i32| {
        w.rules
            .weapon_names
            .get(id.max(0) as usize)
            .filter(|_| id >= 0)
            .cloned()
            .unwrap_or_else(|| "something".into())
    };
    let mut out = Vec::new();
    for e in &w.events {
        match e.kind {
            EVENT_DAMAGE if e.target == me => out.push(format!(
                "TOOK {} damage from #{} (hp now {})",
                e.value, e.actor, w.units[unit].hp
            )),
            EVENT_DAMAGE if e.actor == me => out.push(format!("hit #{} for {}", e.target, e.value)),
            EVENT_KILL if e.target == me && e.actor < 0 => {
                out.push("KILLED by the detonation blast".into())
            }
            EVENT_KILL if e.target == me => {
                out.push(format!("KILLED by #{} with {}", e.actor, weapon(e.value)))
            }
            EVENT_KILL if e.actor == me => {
                out.push(format!("killed #{} with {}", e.target, weapon(e.value)))
            }
            EVENT_PLANT if e.actor == me => out.push("planted the charge".into()),
            EVENT_PLANT => out.push(format!(
                "#{} planted the charge at {}",
                e.actor,
                tiles((e.x, e.y))
            )),
            EVENT_DEFUSE if e.actor == me => out.push("defused the charge".into()),
            EVENT_DEFUSE => out.push(format!("#{} defused the charge", e.actor)),
            EVENT_DETONATE => out.push("the charge detonated".into()),
            EVENT_ROUND_END => out.push(format!(
                "ROUND OVER: team {} wins by {}",
                if e.actor == 0 { "A" } else { "B" },
                reason_name(e.value)
            )),
            _ => {}
        }
    }
    out
}

/// Timeline of `unit` in `round`. By default only ticks where something changed are printed; `all_ticks`
/// prints every tick the unit lived through.
pub fn why(replay: &Replay, round: i32, unit: u32, all_ticks: bool) -> Result<String, String> {
    let mut m = replay.new_match()?;
    let u = unit as usize;
    if u >= m.world.units.len() {
        return Err(format!(
            "no unit {unit}: this match has units 0..{}",
            m.world.units.len() - 1
        ));
    }
    let team = m.world.units[u].team;
    let bot = &replay.header.bots[(team & 1) as usize];
    let mut out = format!(
        "unit {unit} (team {}, bot `{}`), round {round}, seed {}\n",
        if team == 0 { "A" } else { "B" },
        bot.name,
        replay.header.seed
    );
    let (mut seen_before, mut last_phase, mut last_orders, mut printed) =
        (Vec::new(), -1, String::new(), 0usize);
    let mut last_loud: Vec<String> = Vec::new();
    let mut rounds_seen = 0;
    for (i, tick) in replay.ticks.iter().enumerate() {
        let in_round = m.world.round == round;
        rounds_seen = rounds_seen.max(m.world.round);
        let before = in_round.then(|| {
            let w = &m.world;
            let me = &w.units[u];
            let side = if w.side_of(team) == SIDE_ATTACK { "attacking" } else { "defending" };
            let head = format!(
                "t={:<5} {:<9} {:>3} left | {side}, at {} facing {}deg, hp {} armor {} ammo {} credits {}{}",
                w.tick,
                phase_name(w.phase),
                w.phase_left,
                tiles(me.pos),
                deg(me.facing),
                me.hp,
                me.armor,
                me.ammo[me.slot],
                me.credits,
                if me.alive { "" } else { " [dead]" }
            );
            (head, perception(&m, u), w.phase, me.alive)
        });
        let (mut orders, mut moves, mut rejected) = (Vec::new(), Vec::new(), false);
        for e in &tick.events {
            match e {
                Event::Input(Input::Act {
                    unit: who,
                    kind,
                    a,
                    b,
                    c,
                }) if *who == unit && in_round => {
                    let (text, is_move) = describe_act(&m, *kind, *a, *b, *c);
                    let code = Host::act(&mut m, *who, *kind, *a, *b, *c);
                    rejected |= code != OK;
                    let line = if code == OK {
                        text
                    } else {
                        format!("{text} -> REJECTED {}", error_name(code))
                    };
                    if is_move && code == OK {
                        moves.push(line);
                    } else {
                        orders.push(line);
                    }
                }
                Event::Input(input) => m.apply_input(input),
                Event::Fault(who) => {
                    if *who == unit && in_round {
                        orders.push("BOT CRASHED: this unit idles from here on".into());
                    }
                    m.fault(*who);
                }
            }
        }
        m.step();
        if replay.check_tick(i, tick, &m.state_hashes()).is_err() {
            return Err(format!(
                "state hash mismatch after tick {i}: this build does not reproduce the replay"
            ));
        }
        let Some((head, (ids, sees, hears), phase, alive)) = before else {
            continue;
        };
        let after = consequences(&m, u);
        let orders_text = orders.join("; ");
        // Footsteps come and go every tick; only a change in the louder sounds is worth a line.
        let loud: Vec<&str> = hears
            .split(", ")
            .filter(|h| !h.is_empty() && !h.starts_with("steps"))
            .map(|h| h.split(' ').next().unwrap_or(""))
            .collect();
        let interesting = all_ticks
            || ids != seen_before
            || phase != last_phase
            || rejected
            || !after.is_empty()
            || (!orders_text.is_empty() && orders_text != last_orders)
            || (alive && !loud.is_empty() && loud != last_loud);
        if interesting {
            printed += 1;
            let _ = writeln!(out, "{head}");
            let _ = writeln!(
                out,
                "         sees: {}",
                if sees.is_empty() { "no enemy" } else { &sees }
            );
            if !hears.is_empty() {
                let _ = writeln!(out, "         hears: {hears}");
            }
            let all_orders: Vec<&str> = orders.iter().chain(&moves).map(String::as_str).collect();
            let does = if all_orders.is_empty() {
                "nothing".to_string()
            } else {
                all_orders.join("; ")
            };
            let _ = writeln!(out, "         does: {does}");
            for line in &after {
                let _ = writeln!(out, "         => {line}");
            }
        }
        last_loud = loud.iter().map(|s| s.to_string()).collect();
        seen_before = ids;
        last_phase = phase;
        last_orders = orders_text;
    }
    if printed == 0 {
        return Err(format!(
            "round {round} was not played in this replay (it has rounds 1..{rounds_seen})"
        ));
    }
    Ok(out)
}
