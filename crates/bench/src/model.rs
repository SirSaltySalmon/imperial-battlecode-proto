//! What a bench run is made of: bots, maps, a schedule of games, and one result record per game.

use crate::metrics::Detail;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// A participant. A bare reference-bot name (`rusher`) can run natively; a path (bot directory or
/// `.wasm` file) runs through `ibc run-batch`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bot {
    /// Unique display name inside a pool.
    pub label: String,
    /// Name in the `ibc-sim` registry, if this is a built-in reference bot.
    pub native: Option<String>,
    /// Bot directory or `.wasm` file for the wasm backend.
    pub path: Option<PathBuf>,
}

impl Bot {
    /// `spec` is a built-in name or a path. Built-in names also pick up `<bots_dir>/<name>` so they can
    /// play on the wasm backend. Only an exact built-in name is ever run natively: a path that merely
    /// ends in `rusher` may be somebody's modified copy.
    pub fn parse(spec: &str, bots_dir: &Path) -> Result<Bot, String> {
        if ibc_sim::BOTS.contains(&spec) {
            let dir = bots_dir.join(spec);
            return Ok(Bot {
                label: spec.to_string(),
                native: Some(spec.to_string()),
                path: dir.is_dir().then_some(dir),
            });
        }
        let path = PathBuf::from(spec);
        if !path.exists() {
            return Err(format!(
                "`{spec}` is neither a built-in bot ({}) nor an existing bot directory or .wasm file",
                ibc_sim::BOTS.join(", ")
            ));
        }
        // `<bot>/.ibc/bot.wasm` is named after its bot directory, anything else after the file or directory.
        let in_ibc_dir = path
            .parent()
            .and_then(|p| p.file_name())
            .is_some_and(|n| n == ".ibc");
        let named = if in_ibc_dir {
            path.parent().and_then(|p| p.parent()).unwrap_or(&path)
        } else {
            &path
        };
        let label = named
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "bot".into());
        Ok(Bot {
            label,
            native: None,
            path: Some(path),
        })
    }

    /// Parse a pool and make the labels unique (`rusher`, `rusher#2`, ...).
    pub fn parse_pool(specs: &[String], bots_dir: &Path) -> Result<Vec<Bot>, String> {
        let mut bots: Vec<Bot> = Vec::new();
        for spec in specs {
            let mut bot = Bot::parse(spec, bots_dir)?;
            let base = bot.label.clone();
            let mut k = 1;
            while bots.iter().any(|b| b.label == bot.label) {
                k += 1;
                bot.label = format!("{base}#{k}");
            }
            bots.push(bot);
        }
        Ok(bots)
    }
}

#[derive(Clone, Debug)]
pub struct MapEntry {
    pub name: String,
    pub path: PathBuf,
    pub text: String,
}

/// Load one `.ibcmap` file or every `.ibcmap` in a directory, sorted by name.
pub fn load_maps(path: &Path) -> Result<Vec<MapEntry>, String> {
    let mut files = Vec::new();
    if path.is_dir() {
        for entry in std::fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))? {
            let p = entry.map_err(|e| e.to_string())?.path();
            if p.extension().is_some_and(|x| x == "ibcmap") {
                files.push(p);
            }
        }
    } else {
        files.push(path.to_path_buf());
    }
    let mut maps = Vec::new();
    for p in files {
        let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        maps.push(MapEntry {
            name: p
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            path: p,
            text,
        });
    }
    maps.sort_by(|a, b| a.name.cmp(&b.name));
    if maps.is_empty() {
        return Err(format!("no .ibcmap files in {}", path.display()));
    }
    Ok(maps)
}

/// One game to play: `a` sits in the team A slot (attacks first), `b` in team B. Indices into the pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Game {
    pub id: usize,
    pub map: usize,
    pub seed: u64,
    pub a: usize,
    pub b: usize,
}

/// Paired design for bots 0 and 1: every (seed, map) is played twice with the slots swapped. Games
/// `2k` and `2k + 1` form pair `k`; in the even game bot 0 is team A.
pub fn paired_schedule(maps: usize, seeds: &[u64]) -> Vec<Game> {
    let mut games = Vec::with_capacity(maps * seeds.len() * 2);
    for &seed in seeds {
        for map in 0..maps {
            for (a, b) in [(0, 1), (1, 0)] {
                games.push(Game {
                    id: games.len(),
                    map,
                    seed,
                    a,
                    b,
                });
            }
        }
    }
    games
}

/// Every ordered pair of distinct bots on every (seed, map), so each unordered pair meets in both slot
/// orders. With `mirrors`, each bot also plays itself once per (seed, map).
pub fn round_robin_schedule(bots: usize, maps: usize, seeds: &[u64], mirrors: bool) -> Vec<Game> {
    let mut games = Vec::new();
    for &seed in seeds {
        for map in 0..maps {
            for a in 0..bots {
                for b in 0..bots {
                    if a != b || mirrors {
                        games.push(Game {
                            id: games.len(),
                            map,
                            seed,
                            a,
                            b,
                        });
                    }
                }
            }
        }
    }
    games
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoundRec {
    pub round: i32,
    pub winner: i32,
    pub reason: i32,
    pub attackers: i32,
    pub ticks: i32,
    /// The charge was planted. `None` from a summary that does not say (a runner older than the field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planted: Option<bool>,
}

/// One row of the engine summary's `kills`: every `EVENT_KILL`, friendly kills and the blast included.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kill {
    pub tick: u32,
    pub round: i32,
    /// -1 for the detonation blast.
    pub killer: i32,
    pub victim: i32,
    pub killer_team: i32,
    pub victim_team: i32,
    /// Weapon id, -1 for an ability or the blast.
    pub weapon: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamTotals {
    pub kills: i64,
    pub deaths: i64,
    pub damage: i64,
    pub shots: i64,
    pub plants: i64,
    pub defuses: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuelRec {
    pub total: u64,
    pub granted: u64,
    pub max_per_tick: u64,
    pub max_unit_turn: u64,
    pub turns: u64,
    pub out_of_fuel_turns: u64,
}

/// Everything recorded about one game. One JSON line of `games.jsonl`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameResult {
    pub id: usize,
    pub map: String,
    pub seed: u64,
    /// Labels of the bots in the team A and team B slots.
    pub a: String,
    pub b: String,
    /// "native" or "wasm".
    pub backend: String,
    /// Set when the game could not be played or did not finish; such games are excluded from statistics.
    pub error: Option<String>,
    pub winner: Option<i32>,
    pub score: [i32; 2],
    pub ticks: u32,
    pub final_hash: String,
    pub ruleset_hash: String,
    pub rounds: Vec<RoundRec>,
    /// Per team (A, B), summed over its units.
    pub totals: [TeamTotals; 2],
    /// Every kill, from the engine summary. `None` if the summary has no `kills` (an older runner).
    #[serde(default)]
    pub kills: Option<Vec<Kill>>,
    /// The engine summary's `digest`: how the game went, without the seed.
    #[serde(default)]
    pub digest: Option<String>,
    /// Native backend: per team `[act calls, act errors]`.
    pub act: Option<[[u64; 2]; 2]>,
    /// Wasm backend: per team fuel accounting from `summary.json`.
    pub fuel: Option<[FuelRec; 2]>,
    pub faults: [u32; 2],
    /// Wasm backend: where the replay was written.
    pub replay: Option<String>,
    pub detail: Option<Detail>,
}

impl GameResult {
    pub fn failed(
        game: &Game,
        map: &str,
        a: &str,
        b: &str,
        backend: &str,
        error: String,
    ) -> GameResult {
        GameResult {
            id: game.id,
            map: map.to_string(),
            seed: game.seed,
            a: a.to_string(),
            b: b.to_string(),
            backend: backend.to_string(),
            error: Some(error),
            winner: None,
            score: [0, 0],
            ticks: 0,
            final_hash: String::new(),
            ruleset_hash: String::new(),
            rounds: Vec::new(),
            totals: Default::default(),
            kills: None,
            digest: None,
            act: None,
            fuel: None,
            faults: [0, 0],
            replay: None,
            detail: None,
        }
    }

    pub fn ok(&self) -> bool {
        self.error.is_none()
    }

    /// Fill the fields that come from the engine's `summary_json` (same shape on both backends).
    pub fn apply_engine_summary(&mut self, game: &Value) {
        let int = |v: &Value| v.as_i64().unwrap_or(0);
        self.winner = game["winner"].as_i64().map(|w| w as i32);
        self.score = [int(&game["score"][0]) as i32, int(&game["score"][1]) as i32];
        self.ticks = int(&game["ticks"]) as u32;
        self.final_hash = game["final_hash"].as_str().unwrap_or("").to_string();
        self.ruleset_hash = game["ruleset_hash"].as_str().unwrap_or("").to_string();
        self.rounds = game["rounds"]
            .as_array()
            .map(|rounds| {
                rounds
                    .iter()
                    .map(|r| RoundRec {
                        round: int(&r["round"]) as i32,
                        winner: int(&r["winner"]) as i32,
                        reason: int(&r["reason"]) as i32,
                        attackers: int(&r["attackers"]) as i32,
                        ticks: int(&r["ticks"]) as i32,
                        planted: r["planted"].as_bool(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        // [tick, round, killer, victim, killer_team, victim_team, weapon]
        self.kills = game["kills"].as_array().map(|kills| {
            kills
                .iter()
                .map(|k| Kill {
                    tick: int(&k[0]) as u32,
                    round: int(&k[1]) as i32,
                    killer: int(&k[2]) as i32,
                    victim: int(&k[3]) as i32,
                    killer_team: int(&k[4]) as i32,
                    victim_team: int(&k[5]) as i32,
                    weapon: int(&k[6]) as i32,
                })
                .collect()
        });
        self.digest = game["digest"].as_str().map(str::to_string);
        self.totals = Default::default();
        for u in game["units"].as_array().into_iter().flatten() {
            let Some(t) = self.totals.get_mut(int(&u["team"]) as usize) else {
                continue;
            };
            t.kills += int(&u["kills"]);
            t.deaths += int(&u["deaths"]);
            t.damage += int(&u["damage"]);
            t.shots += int(&u["shots"]);
            t.plants += int(&u["plants"]);
            t.defuses += int(&u["defuses"]);
        }
    }

    /// Points for the bot in the given slot (0 = team A): 1 win, 0.5 draw, 0 loss.
    pub fn points(&self, slot: usize) -> f64 {
        match self.winner {
            Some(w) if w as usize == slot => 1.0,
            Some(_) => 0.0,
            None => 0.5,
        }
    }

    /// What happened in the game, without the seed: two games with the same signature are exact repeats
    /// as far as any statistic here can tell. That is the engine summary's `digest` (the final state hash
    /// cannot be used: it covers the seed); a result without one, from an older runner, falls back to its
    /// rounds and totals.
    pub fn signature(&self) -> String {
        match &self.digest {
            Some(digest) => format!("{}|{}|{}|{digest}", self.map, self.a, self.b),
            None => format!(
                "{}|{}|{}|{:?}|{:?}|{}|{:?}|{:?}",
                self.map,
                self.a,
                self.b,
                self.winner,
                self.score,
                self.ticks,
                self.rounds,
                self.totals
            ),
        }
    }

    /// The match ended level on rounds and was decided by the tie-break.
    pub fn tiebreak(&self) -> bool {
        self.winner.is_some() && self.score[0] == self.score[1]
    }
}

/// One warning per bot that ran out of fuel on the wasm backend. Fuel is not modelled natively, so such a
/// bot plays differently (worse) as wasm than its native build does: the two backends then disagree.
pub fn fuel_warnings(results: &[GameResult]) -> Vec<String> {
    let mut per_bot: std::collections::BTreeMap<&str, (u64, u64, u64)> = Default::default();
    for r in results.iter().filter(|r| r.ok()) {
        let Some(fuel) = r.fuel else { continue };
        for (label, f) in [(r.a.as_str(), fuel[0]), (r.b.as_str(), fuel[1])] {
            let e = per_bot.entry(label).or_default();
            e.0 += f.out_of_fuel_turns;
            e.1 += f.turns;
            e.2 += (f.out_of_fuel_turns > 0) as u64;
        }
    }
    per_bot
        .into_iter()
        .filter(|(_, (out, _, _))| *out > 0)
        .map(|(bot, (out, turns, games))| {
            format!(
                "`{bot}` ran out of fuel in {out} of {turns} turns ({games} game sides): those turns were cut short, so its wasm results can differ from the native backend, which does not meter fuel"
            )
        })
        .collect()
}

pub fn write_games_jsonl(path: &Path, results: &[GameResult]) -> Result<(), String> {
    let mut text = String::new();
    for r in results {
        text.push_str(&serde_json::to_string(r).map_err(|e| e.to_string())?);
        text.push('\n');
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn read_games_jsonl(path: &Path) -> Result<Vec<GameResult>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .enumerate()
        .map(|(i, l)| {
            serde_json::from_str(l).map_err(|e| format!("{}:{}: {e}", path.display(), i + 1))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_schedule_swaps_slots_on_the_same_map_and_seed() {
        let games = paired_schedule(2, &[5, 9, 11]);
        assert_eq!(games.len(), 12);
        for (k, pair) in games.chunks(2).enumerate() {
            let (x, y) = (pair[0], pair[1]);
            assert_eq!((x.id, y.id), (2 * k, 2 * k + 1));
            assert_eq!(
                (x.map, x.seed),
                (y.map, y.seed),
                "a pair shares map and seed"
            );
            assert_eq!((x.a, x.b), (0, 1));
            assert_eq!((y.a, y.b), (1, 0));
        }
        // Every (seed, map) appears exactly once as a pair, seeds in the order given.
        let keys: Vec<(u64, usize)> = games.chunks(2).map(|p| (p[0].seed, p[0].map)).collect();
        assert_eq!(keys, vec![(5, 0), (5, 1), (9, 0), (9, 1), (11, 0), (11, 1)]);
        // A longer seed list only appends: the SPRT can stop early and stay a prefix of the full run.
        assert_eq!(paired_schedule(2, &[5, 9])[..], games[..8]);
    }

    #[test]
    fn round_robin_covers_both_slot_orders() {
        let games = round_robin_schedule(3, 2, &[1, 2], false);
        assert_eq!(games.len(), 3 * 2 * 2 * 2);
        for a in 0..3 {
            for b in 0..3 {
                let n = games.iter().filter(|g| g.a == a && g.b == b).count();
                assert_eq!(n, if a == b { 0 } else { 4 });
            }
        }
        assert!(games.iter().enumerate().all(|(i, g)| g.id == i));
        assert_eq!(round_robin_schedule(3, 2, &[1, 2], true).len(), 9 * 4);
    }

    #[test]
    fn pool_labels_are_unique() {
        let specs = vec![
            "rusher".to_string(),
            "rusher".to_string(),
            "holder".to_string(),
        ];
        let pool = Bot::parse_pool(&specs, Path::new("no-such-dir")).unwrap();
        let labels: Vec<&str> = pool.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(labels, ["rusher", "rusher#2", "holder"]);
        assert!(pool.iter().all(|b| b.native.is_some() && b.path.is_none()));
        assert!(Bot::parse("definitely-not-a-bot", Path::new("no-such-dir")).is_err());
    }
}
