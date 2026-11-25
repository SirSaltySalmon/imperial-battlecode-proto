//! `ibc-bench balance`: the game designer's report, computed from league games that carry per-tick details.
//!
//! Every headline number is a ratio of per-match sums with a cluster interval (matches are the
//! independent unit, rounds inside a match are not), widened when the seed list produced exact repeats.
//! `report.json` keeps all of them in one flat `metrics` map so `--compare` can diff two runs.

use crate::league::distinct_games;
use crate::model::GameResult;
use crate::stats::{percentile, ratio_ci, Est};
use ibc_abi::{
    ROUND_WIN_DEFUSE, ROUND_WIN_DETONATION, ROUND_WIN_ELIMINATION, ROUND_WIN_TIMEOUT, TILE,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const SCHEMA: &str = "ibc-bench/balance/1";

/// Kill distance bands in tiles: label and exclusive upper bound.
pub(crate) const BANDS: [(&str, f64); 5] = [
    ("0-2", 2.0),
    ("2-5", 5.0),
    ("5-10", 10.0),
    ("10-20", 20.0),
    ("20+", f64::INFINITY),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistBin {
    pub from: i32,
    pub to: i32,
    pub rounds: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EconRow {
    pub round: i32,
    pub rounds: u64,
    /// Mean credits per unit at the first buy tick and mean credits spent per unit in the buy phase.
    pub attack_credits: f64,
    pub defend_credits: f64,
    pub attack_spent: f64,
    pub defend_spent: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MatchupRow {
    pub attacker: String,
    pub defender: String,
    pub rounds: u64,
    pub attacker_wins: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FuelRow {
    pub bot: String,
    pub games: u64,
    /// Percentiles over games of the mean fuel per bot turn.
    pub mean_per_turn_p50: f64,
    pub mean_per_turn_p90: f64,
    pub mean_per_turn_max: f64,
    /// Most fuel any unit used in one turn, over all games.
    pub max_unit_turn: u64,
    /// Share of the granted budget that was used.
    pub budget_used: f64,
    pub turns: u64,
    pub out_of_fuel_turns: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Degenerate {
    pub zero_shot_rounds: u64,
    pub zero_move_rounds: u64,
    /// Pairings (attacker slot bot vs other bot on a map) in which every round ended on the timer.
    pub always_timeout: Vec<String>,
    /// Pairings in which nobody fired a single shot in any game.
    pub never_shoot: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BalanceReport {
    pub schema: String,
    pub backend: String,
    pub ruleset_hash: String,
    pub bots: Vec<String>,
    pub maps: Vec<String>,
    pub seeds: u64,
    pub games: u64,
    pub failed: u64,
    pub distinct_games: u64,
    pub rounds: u64,
    /// Flat map of every comparable number. Rates and shares are fractions in 0..=1.
    pub metrics: BTreeMap<String, Est>,
    pub round_length_histogram: Vec<HistBin>,
    pub economy_by_round: Vec<EconRow>,
    /// Share of units holding each weapon when the action phase starts: side -> weapon -> share.
    pub loadouts: BTreeMap<String, BTreeMap<String, f64>>,
    pub attack_matrix: Vec<MatchupRow>,
    pub fuel: Vec<FuelRow>,
    pub degenerate: Degenerate,
    pub warnings: Vec<String>,
}

pub fn reason_name(reason: i32) -> String {
    match reason {
        ROUND_WIN_ELIMINATION => "elimination".into(),
        ROUND_WIN_DETONATION => "detonation".into(),
        ROUND_WIN_DEFUSE => "defuse".into(),
        ROUND_WIN_TIMEOUT => "timeout".into(),
        other => format!("reason_{other}"),
    }
}

fn weapon_name(names: &[String], id: i32) -> String {
    match names.get(id.max(0) as usize) {
        Some(n) if id >= 0 && !n.is_empty() => n.clone(),
        _ if id < 0 => "other".into(),
        _ => format!("weapon_{id}"),
    }
}

fn band_of(dist: i32) -> &'static str {
    let tiles = dist as f64 / TILE as f64;
    BANDS
        .iter()
        .find(|b| tiles < b.1)
        .map(|b| b.0)
        .unwrap_or("20+")
}

/// Collects metrics over a set of games; `widen` stretches intervals when games repeat exactly.
struct Metrics<'a> {
    games: &'a [&'a GameResult],
    widen: f64,
    distinct: u64,
    out: BTreeMap<String, Est>,
}

impl Metrics<'_> {
    /// `sum(num) / sum(den)` over games. `unit_interval` clamps the interval to 0..=1 (rates and shares).
    fn add(&mut self, key: &str, unit_interval: bool, f: impl Fn(&GameResult) -> (f64, f64)) {
        let clusters: Vec<(f64, f64)> = self
            .games
            .iter()
            .map(|g| f(g))
            .filter(|c| c.1 > 0.0 || c.0 != 0.0)
            .collect();
        let Some(mut e) = ratio_ci(&clusters) else {
            return;
        };
        if let (Some(lo), Some(hi)) = (e.lo, e.hi) {
            let half = (hi - lo) / 2.0 * self.widen;
            (e.lo, e.hi) = (Some(e.value - half), Some(e.value + half));
        }
        e.n = e.n.min(self.distinct);
        self.out.insert(
            key.to_string(),
            if unit_interval {
                e.clamped(0.0, 1.0)
            } else {
                e
            },
        );
    }
}

fn rounds_where(g: &GameResult, f: impl Fn(&crate::metrics::RoundDetail) -> bool) -> f64 {
    g.detail
        .as_ref()
        .map_or(0.0, |d| d.rounds.iter().filter(|r| f(r)).count() as f64)
}

fn detail_sum(g: &GameResult, f: impl Fn(&crate::metrics::RoundDetail) -> f64) -> f64 {
    g.detail
        .as_ref()
        .map_or(0.0, |d| d.rounds.iter().map(f).sum())
}

fn detail_rounds(g: &GameResult) -> f64 {
    g.detail.as_ref().map_or(0.0, |d| d.rounds.len() as f64)
}

fn core_metrics(m: &mut Metrics, maps: &[String]) {
    let attack_wins =
        |g: &GameResult| g.rounds.iter().filter(|r| r.winner == r.attackers).count() as f64;
    m.add("attacker_round_win_rate", true, |g| {
        (attack_wins(g), g.rounds.len() as f64)
    });
    for map in maps {
        m.add(&format!("attacker_round_win_rate.map.{map}"), true, |g| {
            if &g.map == map {
                (attack_wins(g), g.rounds.len() as f64)
            } else {
                (0.0, 0.0)
            }
        });
    }
    let mut reasons: BTreeSet<i32> = [
        ROUND_WIN_ELIMINATION,
        ROUND_WIN_DETONATION,
        ROUND_WIN_DEFUSE,
        ROUND_WIN_TIMEOUT,
    ]
    .into_iter()
    .collect();
    reasons.extend(
        m.games
            .iter()
            .flat_map(|g| g.rounds.iter().map(|r| r.reason)),
    );
    for reason in reasons {
        m.add(
            &format!("end_reason.{}.share", reason_name(reason)),
            true,
            |g| {
                (
                    g.rounds.iter().filter(|r| r.reason == reason).count() as f64,
                    g.rounds.len() as f64,
                )
            },
        );
    }
    m.add("round_ticks.mean", false, |g| {
        (
            g.rounds.iter().map(|r| r.ticks as f64).sum(),
            g.rounds.len() as f64,
        )
    });
    m.add("match_ticks.mean", false, |g| (g.ticks as f64, 1.0));
    m.add("match_rounds.mean", false, |g| (g.rounds.len() as f64, 1.0));
    m.add("draw_rate", true, |g| {
        (g.winner.is_none() as u8 as f64, 1.0)
    });
    m.add("tiebreak_rate", true, |g| (g.tiebreak() as u8 as f64, 1.0));
    m.add("team_a_slot_score", true, |g| (g.points(0), 1.0));
    let total = |g: &GameResult, f: fn(&crate::model::TeamTotals) -> i64| {
        (f(&g.totals[0]) + f(&g.totals[1])) as f64
    };
    m.add("shots_per_kill", false, |g| {
        (total(g, |t| t.shots), total(g, |t| t.kills))
    });
    m.add("damage_per_shot", false, |g| {
        (total(g, |t| t.damage), total(g, |t| t.shots))
    });
    m.add("shots_per_round", false, |g| {
        (total(g, |t| t.shots), g.rounds.len() as f64)
    });
    m.add("kills_per_round", false, |g| {
        (total(g, |t| t.kills), g.rounds.len() as f64)
    });
    m.add("damage_per_round", false, |g| {
        (total(g, |t| t.damage), g.rounds.len() as f64)
    });
}

fn detail_metrics(m: &mut Metrics) {
    m.add("plant_rate", true, |g| {
        (rounds_where(g, |r| r.planted), detail_rounds(g))
    });
    m.add("first_contact_ticks.mean", false, |g| {
        (
            detail_sum(g, |r| r.first_contact.unwrap_or(0) as f64),
            rounds_where(g, |r| r.first_contact.is_some()),
        )
    });
    m.add("first_kill_ticks.mean", false, |g| {
        (
            detail_sum(g, |r| r.first_kill.unwrap_or(0) as f64),
            rounds_where(g, |r| r.first_kill.is_some()),
        )
    });
    m.add("no_contact_rounds.share", true, |g| {
        (
            rounds_where(g, |r| r.first_contact.is_none()),
            detail_rounds(g),
        )
    });
    m.add("no_kill_rounds.share", true, |g| {
        (
            rounds_where(g, |r| r.first_kill.is_none()),
            detail_rounds(g),
        )
    });
    m.add("degenerate.zero_shot_rounds.share", true, |g| {
        (rounds_where(g, |r| r.shots == 0), detail_rounds(g))
    });
    m.add("degenerate.zero_move_rounds.share", true, |g| {
        (rounds_where(g, |r| r.moved == 0), detail_rounds(g))
    });
    let units = |r: &crate::metrics::RoundDetail, side: usize| r.loadout[side].len() as f64;
    m.add("moved_tiles_per_unit_round", false, |g| {
        (
            detail_sum(g, |r| r.moved as f64 / TILE as f64),
            detail_sum(g, |r| units(r, 0) + units(r, 1)),
        )
    });
    for (side, name) in ["attack", "defend"].into_iter().enumerate() {
        m.add(&format!("econ.spent_per_unit.{name}"), false, |g| {
            (
                detail_sum(g, |r| r.spent[side] as f64),
                detail_sum(g, |r| units(r, side)),
            )
        });
        m.add(&format!("econ.credits_per_unit.{name}"), false, |g| {
            (
                detail_sum(g, |r| r.credits_start[side] as f64),
                detail_sum(g, |r| units(r, side)),
            )
        });
    }
    let kills = |g: &GameResult| g.detail.as_ref().map_or(0.0, |d| d.kills.len() as f64);
    let kills_where = |g: &GameResult, f: &dyn Fn(&crate::metrics::KillRec) -> bool| {
        g.detail
            .as_ref()
            .map_or(0.0, |d| d.kills.iter().filter(|k| f(k)).count() as f64)
    };
    m.add("kill_distance_tiles.mean", false, |g| {
        let sum = g.detail.as_ref().map_or(0.0, |d| {
            d.kills
                .iter()
                .filter(|k| k.dist >= 0)
                .map(|k| k.dist as f64 / TILE as f64)
                .sum()
        });
        (sum, kills_where(g, &|k| k.dist >= 0))
    });
    for (band, _) in BANDS {
        m.add(&format!("kills.distance.{band}.share"), true, |g| {
            (
                kills_where(g, &|k| k.dist >= 0 && band_of(k.dist) == band),
                kills_where(g, &|k| k.dist >= 0),
            )
        });
    }
    // Weapons come from the games themselves, never from a hard-coded list.
    let mut weapons: BTreeSet<(i32, String)> = BTreeSet::new();
    for g in m.games {
        if let Some(d) = &g.detail {
            weapons.extend(
                d.kills
                    .iter()
                    .map(|k| (k.weapon.max(-1), weapon_name(&d.weapon_names, k.weapon))),
            );
            weapons.extend(
                d.shots_by_weapon
                    .iter()
                    .enumerate()
                    .filter(|s| *s.1 > 0)
                    .map(|(i, _)| (i as i32, weapon_name(&d.weapon_names, i as i32))),
            );
        }
    }
    for (id, name) in weapons {
        m.add(&format!("kills.weapon.{name}.share"), true, |g| {
            (kills_where(g, &|k| k.weapon.max(-1) == id), kills(g))
        });
        if id >= 0 {
            m.add(&format!("shots_per_kill.weapon.{name}"), false, |g| {
                let shots = g
                    .detail
                    .as_ref()
                    .and_then(|d| d.shots_by_weapon.get(id as usize))
                    .copied()
                    .unwrap_or(0);
                (shots as f64, kills_where(g, &|k| k.weapon == id))
            });
        }
    }
}

/// Per bot label: `(slot, game)` for every game it played on the wasm backend.
fn fuel_rows(games: &[&GameResult], m: &mut Metrics) -> Vec<FuelRow> {
    let mut bots: BTreeSet<&str> = BTreeSet::new();
    for g in games.iter().filter(|g| g.fuel.is_some()) {
        bots.extend([g.a.as_str(), g.b.as_str()]);
    }
    let mut rows = Vec::new();
    for bot in bots {
        let recs: Vec<crate::model::FuelRec> = games
            .iter()
            .filter_map(|g| g.fuel.map(|f| (g, f)))
            .flat_map(|(g, f)| [(g.a.as_str(), f[0]), (g.b.as_str(), f[1])])
            .filter(|(label, _)| *label == bot)
            .map(|(_, f)| f)
            .collect();
        let mut means: Vec<f64> = recs
            .iter()
            .filter(|f| f.turns > 0)
            .map(|f| f.total as f64 / f.turns as f64)
            .collect();
        means.sort_by(f64::total_cmp);
        let sum = |f: fn(&crate::model::FuelRec) -> u64| recs.iter().map(f).sum::<u64>();
        rows.push(FuelRow {
            bot: bot.to_string(),
            games: recs.len() as u64,
            mean_per_turn_p50: percentile(&means, 50.0),
            mean_per_turn_p90: percentile(&means, 90.0),
            mean_per_turn_max: means.last().copied().unwrap_or(0.0),
            max_unit_turn: recs.iter().map(|f| f.max_unit_turn).max().unwrap_or(0),
            budget_used: sum(|f| f.total) as f64 / sum(|f| f.granted).max(1) as f64,
            turns: sum(|f| f.turns),
            out_of_fuel_turns: sum(|f| f.out_of_fuel_turns),
        });
        let of_bot = |g: &GameResult, f: fn(&crate::model::FuelRec) -> u64| {
            let fuel = g.fuel.unwrap_or_default();
            ((g.a == bot) as u64 * f(&fuel[0]) + (g.b == bot) as u64 * f(&fuel[1])) as f64
        };
        m.add(&format!("fuel.{bot}.mean_per_turn"), false, |g| {
            (of_bot(g, |f| f.total), of_bot(g, |f| f.turns))
        });
        m.add(&format!("fuel.{bot}.budget_used"), true, |g| {
            (of_bot(g, |f| f.total), of_bot(g, |f| f.granted))
        });
        m.add(&format!("fuel.{bot}.out_of_fuel_turn_rate"), true, |g| {
            (of_bot(g, |f| f.out_of_fuel_turns), of_bot(g, |f| f.turns))
        });
    }
    rows
}

fn histogram(games: &[&GameResult]) -> Vec<HistBin> {
    let ticks: Vec<i32> = games
        .iter()
        .flat_map(|g| g.rounds.iter().map(|r| r.ticks.max(0)))
        .collect();
    let Some(&max) = ticks.iter().max() else {
        return Vec::new();
    };
    // At most ten bins, widths in multiples of ten ticks.
    let width = ((max / 10) / 10 + 1) * 10;
    (0..=max / width)
        .map(|i| HistBin {
            from: i * width,
            to: (i + 1) * width - 1,
            rounds: ticks.iter().filter(|&&t| t / width == i).count() as u64,
        })
        .collect()
}

fn economy(games: &[&GameResult]) -> Vec<EconRow> {
    // round -> (rounds, units per side, credits per side, spent per side)
    let mut acc: BTreeMap<i32, (u64, [f64; 2], [f64; 2], [f64; 2])> = BTreeMap::new();
    for r in games
        .iter()
        .filter_map(|g| g.detail.as_ref())
        .flat_map(|d| d.rounds.iter())
    {
        let e = acc.entry(r.round).or_default();
        e.0 += 1;
        for side in 0..2 {
            e.1[side] += r.loadout[side].len() as f64;
            e.2[side] += r.credits_start[side] as f64;
            e.3[side] += r.spent[side] as f64;
        }
    }
    acc.into_iter()
        .map(|(round, (rounds, units, credits, spent))| EconRow {
            round,
            rounds,
            attack_credits: credits[0] / units[0].max(1.0),
            defend_credits: credits[1] / units[1].max(1.0),
            attack_spent: spent[0] / units[0].max(1.0),
            defend_spent: spent[1] / units[1].max(1.0),
        })
        .collect()
}

fn loadouts(games: &[&GameResult]) -> BTreeMap<String, BTreeMap<String, f64>> {
    let mut out = BTreeMap::new();
    for (side, name) in ["attack", "defend"].into_iter().enumerate() {
        let mut counts: BTreeMap<String, f64> = BTreeMap::new();
        for d in games.iter().filter_map(|g| g.detail.as_ref()) {
            for &w in d.rounds.iter().flat_map(|r| r.loadout[side].iter()) {
                *counts.entry(weapon_name(&d.weapon_names, w)).or_default() += 1.0;
            }
        }
        let total: f64 = counts.values().sum();
        counts.values_mut().for_each(|c| *c /= total.max(1.0));
        out.insert(name.to_string(), counts);
    }
    out
}

fn attack_matrix(games: &[&GameResult]) -> Vec<MatchupRow> {
    let mut acc: BTreeMap<(String, String), (u64, u64)> = BTreeMap::new();
    for g in games {
        for r in &g.rounds {
            let (attacker, defender) = if r.attackers == 0 {
                (&g.a, &g.b)
            } else {
                (&g.b, &g.a)
            };
            let e = acc.entry((attacker.clone(), defender.clone())).or_default();
            e.0 += 1;
            e.1 += (r.winner == r.attackers) as u64;
        }
    }
    acc.into_iter()
        .map(
            |((attacker, defender), (rounds, attacker_wins))| MatchupRow {
                attacker,
                defender,
                rounds,
                attacker_wins,
            },
        )
        .collect()
}

fn degenerate(games: &[&GameResult]) -> Degenerate {
    let mut d = Degenerate::default();
    // pairing -> (rounds, timeouts, shots)
    let mut pairings: BTreeMap<String, (u64, u64, i64)> = BTreeMap::new();
    for g in games {
        let e = pairings
            .entry(format!("{} vs {} on {}", g.a, g.b, g.map))
            .or_default();
        e.0 += g.rounds.len() as u64;
        e.1 += g
            .rounds
            .iter()
            .filter(|r| r.reason == ROUND_WIN_TIMEOUT)
            .count() as u64;
        e.2 += g.totals[0].shots + g.totals[1].shots;
        if let Some(detail) = &g.detail {
            d.zero_shot_rounds += detail.rounds.iter().filter(|r| r.shots == 0).count() as u64;
            d.zero_move_rounds += detail.rounds.iter().filter(|r| r.moved == 0).count() as u64;
        }
    }
    for (name, (rounds, timeouts, shots)) in pairings {
        if rounds > 0 && timeouts == rounds {
            d.always_timeout.push(format!("{name} ({rounds} rounds)"));
        }
        if rounds > 0 && shots == 0 {
            d.never_shoot.push(format!("{name} ({rounds} rounds)"));
        }
    }
    d
}

/// Build the report from league results. Games without details still contribute the summary-level metrics.
pub fn build(results: &[GameResult], mut warnings: Vec<String>) -> BalanceReport {
    let ok: Vec<&GameResult> = results.iter().filter(|r| r.ok()).collect();
    let failed = (results.len() - ok.len()) as u64;
    let distinct = distinct_games(results).len() as u64;
    if failed > 0 {
        warnings.push(format!("{failed} game(s) failed and are excluded"));
    }
    if distinct < ok.len() as u64 {
        warnings.push(format!(
            "{} of {} games are exact repeats (same map, slots, rounds and totals): the seed does not change \
             those games. Intervals are widened to the {distinct} distinct games; more maps or bots add information, \
             more seeds do not",
            ok.len() as u64 - distinct,
            ok.len()
        ));
    }
    warnings.extend(crate::model::fuel_warnings(results));
    let without_detail = ok.iter().filter(|g| g.detail.is_none()).count();
    if without_detail > 0 {
        warnings.push(format!(
            "{without_detail} game(s) carry no per-tick details; detail metrics use the rest"
        ));
    }
    let uniq = |f: &dyn Fn(&GameResult) -> String| {
        ok.iter()
            .map(|g| f(g))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    };
    let maps = uniq(&|g| g.map.clone());
    let mut bots = uniq(&|g| g.a.clone());
    bots.extend(uniq(&|g| g.b.clone()));
    bots.sort();
    bots.dedup();

    let widen = if distinct > 0 {
        (ok.len() as f64 / distinct as f64).sqrt()
    } else {
        1.0
    };
    let mut m = Metrics {
        games: &ok,
        widen,
        distinct,
        out: BTreeMap::new(),
    };
    core_metrics(&mut m, &maps);
    detail_metrics(&mut m);
    let fuel = fuel_rows(&ok, &mut m);
    let degenerate = degenerate(&ok);
    if !degenerate.always_timeout.is_empty() || !degenerate.never_shoot.is_empty() {
        warnings.push(format!(
            "degenerate pairings: {} always time out, {} never shoot (listed below). A pool with passive bots \
             says little about balance",
            degenerate.always_timeout.len(),
            degenerate.never_shoot.len()
        ));
    }
    BalanceReport {
        schema: SCHEMA.into(),
        backend: uniq(&|g| g.backend.clone()).join("+"),
        ruleset_hash: ok
            .first()
            .map(|g| g.ruleset_hash.clone())
            .unwrap_or_default(),
        bots,
        maps,
        seeds: ok.iter().map(|g| g.seed).collect::<BTreeSet<_>>().len() as u64,
        games: ok.len() as u64,
        failed,
        distinct_games: distinct,
        rounds: ok.iter().map(|g| g.rounds.len() as u64).sum(),
        metrics: m.out,
        round_length_histogram: histogram(&ok),
        economy_by_round: economy(&ok),
        loadouts: loadouts(&ok),
        attack_matrix: attack_matrix(&ok),
        fuel,
        degenerate,
        warnings,
    }
}

pub use crate::balance_render::{format_metric, is_fraction, render_markdown, render_summary};
