//! `ibc-bench league`: round-robin of a bot pool, cross table and Bradley-Terry ratings with bootstrap
//! intervals. Also the data source of `balance`.

use crate::backend::{run_games, Run, RunOpts};
use crate::model::{round_robin_schedule, Bot, GameResult, MapEntry};
use crate::report::{est_num, md_table, pct};
use crate::stats::{bradley_terry, percentile, Est, Rng};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const SCHEMA: &str = "ibc-bench/league/1";
const BOOTSTRAP_ROUNDS: usize = 400;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LeagueRow {
    pub bot: String,
    pub games: u64,
    pub wins: u64,
    pub draws: u64,
    pub losses: u64,
    pub score: f64,
    /// Bradley-Terry rating on the Elo scale, pool mean 0, with a bootstrap interval over (map, seed) blocks.
    pub elo: Est,
    pub rounds_for: i64,
    pub rounds_against: i64,
    /// Native backend only: share of `act` calls that returned an error.
    pub act_error_rate: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrossCell {
    /// Points the row bot scored against the column bot (win 1, draw 0.5), both slot orders together.
    pub points: f64,
    pub games: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LeagueReport {
    pub schema: String,
    pub backend: String,
    pub ruleset_hash: String,
    pub bots: Vec<String>,
    pub maps: Vec<String>,
    pub seeds: Vec<u64>,
    pub games: u64,
    pub failed: u64,
    /// Games left after dropping exact repeats (same map, bots and slots, same game digest).
    pub distinct_games: u64,
    /// Sorted by rating, best first.
    pub rows: Vec<LeagueRow>,
    /// `cross[i][j]` for `bots[i]` against `bots[j]`; `None` on the diagonal or when they never met.
    pub cross: Vec<Vec<Option<CrossCell>>>,
    pub warnings: Vec<String>,
}

pub fn run_league(
    bots: &[Bot],
    maps: &[MapEntry],
    seeds: &[u64],
    mirrors: bool,
    opts: &RunOpts,
) -> Result<Run, String> {
    if bots.len() < 2 {
        return Err("a league needs at least two bots".into());
    }
    run_games(
        bots,
        maps,
        &round_robin_schedule(bots.len(), maps.len(), seeds, mirrors),
        opts,
    )
}

/// Exact repeats removed: the same bots in the same slots on the same map, and the same game (`signature`).
pub fn distinct_games(results: &[GameResult]) -> Vec<&GameResult> {
    let mut seen = BTreeSet::new();
    results
        .iter()
        .filter(|r| r.ok() && seen.insert(r.signature()))
        .collect()
}

fn points_matrix(bots: &[String], games: &[&GameResult]) -> Vec<Vec<f64>> {
    let index: BTreeMap<&str, usize> = bots
        .iter()
        .enumerate()
        .map(|(i, b)| (b.as_str(), i))
        .collect();
    let mut m = vec![vec![0.0; bots.len()]; bots.len()];
    for r in games {
        if let (Some(&i), Some(&j)) = (index.get(r.a.as_str()), index.get(r.b.as_str())) {
            if i != j {
                m[i][j] += r.points(0);
                m[j][i] += r.points(1);
            }
        }
    }
    m
}

/// Percentile bootstrap of the ratings, resampling whole (map, seed) blocks: games of one block share the
/// map and seed, so they are not independent draws. Blocks that are exact repeats are counted once.
fn elo_intervals(bots: &[String], games: &[&GameResult]) -> Vec<(Option<f64>, Option<f64>, u64)> {
    let mut blocks: BTreeMap<(&str, u64), Vec<&GameResult>> = BTreeMap::new();
    for &r in games {
        blocks.entry((r.map.as_str(), r.seed)).or_default().push(r);
    }
    let mut seen = BTreeSet::new();
    let blocks: Vec<Vec<&GameResult>> = blocks
        .into_iter()
        .filter(|(_, games)| {
            let mut sig: Vec<String> = games.iter().map(|r| r.signature()).collect();
            sig.sort();
            seen.insert(sig)
        })
        .map(|(_, games)| games)
        .collect();
    if blocks.len() < 2 {
        return vec![(None, None, blocks.len() as u64); bots.len()];
    }
    let mut rng = Rng(0x1BC_BE4C);
    let mut samples: Vec<Vec<f64>> = vec![Vec::with_capacity(BOOTSTRAP_ROUNDS); bots.len()];
    for _ in 0..BOOTSTRAP_ROUNDS {
        let drawn: Vec<&GameResult> = (0..blocks.len())
            .flat_map(|_| blocks[rng.below(blocks.len())].iter().copied())
            .collect();
        for (i, elo) in bradley_terry(&points_matrix(bots, &drawn))
            .into_iter()
            .enumerate()
        {
            samples[i].push(elo);
        }
    }
    samples
        .into_iter()
        .map(|mut s| {
            s.sort_by(f64::total_cmp);
            (
                Some(percentile(&s, 2.5)),
                Some(percentile(&s, 97.5)),
                blocks.len() as u64,
            )
        })
        .collect()
}

/// Build the league table from results (mirror games are ignored for standings).
pub fn table(bots: &[String], results: &[GameResult], mut warnings: Vec<String>) -> LeagueReport {
    let ok: Vec<&GameResult> = results.iter().filter(|r| r.ok() && r.a != r.b).collect();
    let failed = results.iter().filter(|r| !r.ok()).count() as u64;
    let distinct = distinct_games(results).len() as u64;
    let played = results.iter().filter(|r| r.ok()).count() as u64;
    if failed > 0 {
        warnings.push(format!("{failed} game(s) failed and are excluded"));
    }
    if distinct < played {
        warnings.push(format!(
            "{} of {played} games are exact repeats (same map, slots and game digest): the seed does not \
             change those games. Rating intervals count each distinct (map, seed) block once",
            played - distinct
        ));
    }
    warnings.extend(crate::model::fuel_warnings(results));
    let points = points_matrix(bots, &ok);
    let elo = bradley_terry(&points);
    let intervals = elo_intervals(bots, &ok);
    if intervals.iter().any(|i| i.0.is_some() && i.0 == i.1) {
        warnings.push(
            "a zero-width rating interval means every (map, seed) block gave the same results for that bot: the pool or map list is too small to show any variation, which is not the same as certainty"
                .into(),
        );
    }
    let mut cross: Vec<Vec<Option<CrossCell>>> = vec![vec![None; bots.len()]; bots.len()];
    let mut rows: Vec<LeagueRow> = bots
        .iter()
        .enumerate()
        .map(|(i, bot)| {
            let mut row = LeagueRow {
                bot: bot.clone(),
                games: 0,
                wins: 0,
                draws: 0,
                losses: 0,
                score: 0.5,
                elo: Est {
                    value: elo[i],
                    lo: intervals[i].0,
                    hi: intervals[i].1,
                    n: intervals[i].2,
                },
                rounds_for: 0,
                rounds_against: 0,
                act_error_rate: None,
            };
            let (mut calls, mut errors) = (0u64, 0u64);
            for r in &ok {
                let slot = match (&r.a == bot, &r.b == bot) {
                    (true, _) => 0,
                    (_, true) => 1,
                    _ => continue,
                };
                let p = r.points(slot);
                row.games += 1;
                row.wins += (p == 1.0) as u64;
                row.draws += (p == 0.5) as u64;
                row.losses += (p == 0.0) as u64;
                row.rounds_for += r.score[slot] as i64;
                row.rounds_against += r.score[1 - slot] as i64;
                if let Some(act) = r.act {
                    calls += act[slot][0];
                    errors += act[slot][1];
                }
                let other = if slot == 0 { &r.b } else { &r.a };
                if let Some(j) = bots.iter().position(|b| b == other) {
                    let cell = cross[i][j].get_or_insert(CrossCell {
                        points: 0.0,
                        games: 0,
                    });
                    cell.points += p;
                    cell.games += 1;
                }
            }
            if row.games > 0 {
                row.score = (row.wins as f64 + row.draws as f64 / 2.0) / row.games as f64;
            }
            row.act_error_rate = (calls > 0).then(|| errors as f64 / calls as f64);
            row
        })
        .collect();
    rows.sort_by(|x, y| y.elo.value.total_cmp(&x.elo.value));
    let uniq = |f: &dyn Fn(&GameResult) -> String| {
        let mut v: Vec<String> = Vec::new();
        for r in results {
            let x = f(r);
            if !v.contains(&x) {
                v.push(x);
            }
        }
        v
    };
    let mut seeds: Vec<u64> = Vec::new();
    for r in results {
        if !seeds.contains(&r.seed) {
            seeds.push(r.seed);
        }
    }
    let mut backends = uniq(&|r| r.backend.clone());
    backends.sort();
    LeagueReport {
        schema: SCHEMA.into(),
        backend: backends.join("+"),
        ruleset_hash: ok
            .first()
            .map(|r| r.ruleset_hash.clone())
            .unwrap_or_default(),
        bots: bots.to_vec(),
        maps: uniq(&|r| r.map.clone()),
        seeds,
        games: played,
        failed,
        distinct_games: distinct,
        rows,
        cross,
        warnings,
    }
}

pub fn render_markdown(r: &LeagueReport) -> String {
    let mut s = format!(
        "# League\n\n- backend `{}`, ruleset `{}`, maps: {}, seeds: {}\n- {} games ({} distinct, {} failed)\n\n",
        r.backend,
        r.ruleset_hash,
        r.maps.join(", "),
        r.seeds.len(),
        r.games,
        r.distinct_games,
        r.failed
    );
    for w in &r.warnings {
        s.push_str(&format!("> WARNING: {w}\n\n"));
    }
    let rows: Vec<Vec<String>> = r
        .rows
        .iter()
        .map(|row| {
            vec![
                row.bot.clone(),
                est_num(&row.elo, 0),
                row.games.to_string(),
                format!("{}-{}-{}", row.wins, row.draws, row.losses),
                pct(row.score),
                format!("{}-{}", row.rounds_for, row.rounds_against),
                row.act_error_rate.map(pct).unwrap_or_else(|| "n/a".into()),
            ]
        })
        .collect();
    s.push_str(&md_table(
        &[
            "bot",
            "Elo (95 %)",
            "games",
            "W-D-L",
            "score",
            "rounds",
            "act errors",
        ],
        &rows,
    ));
    s.push_str("\n## Cross table (points of the row bot against the column bot / games)\n\n");
    let mut headers: Vec<&str> = vec![""];
    headers.extend(r.bots.iter().map(String::as_str));
    let rows: Vec<Vec<String>> = r
        .bots
        .iter()
        .enumerate()
        .map(|(i, bot)| {
            let mut row = vec![bot.clone()];
            row.extend(r.cross[i].iter().map(|c| match c {
                Some(c) => format!("{}/{}", c.points, c.games),
                None => "-".into(),
            }));
            row
        })
        .collect();
    s.push_str(&md_table(&headers, &rows));
    s.push_str(
        "\nRatings are Bradley-Terry strengths on the Elo scale (pool mean 0, one virtual draw per pairing so \
         sweeps stay finite). Overlapping intervals mean the order of those bots is not established.\n",
    );
    s
}
