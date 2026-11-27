//! `ibc-bench ab`: is bot A better than bot B? Paired design: every (map, seed) is played twice with the
//! slots swapped, so map and side effects cancel inside a pair and the pair is the unit of evidence.

use crate::backend::{run_games, RunOpts};
use crate::model::{paired_schedule, Bot, GameResult, MapEntry};
use crate::report::{backend_label, est_num, est_pct, md_table, p_value, pct};
use crate::stats::{
    mean_var, score_to_elo, sign_test_p, t95, wilson, Est, Sprt, SprtDecision, Z95,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const SCHEMA: &str = "ibc-bench/ab/1";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PairStats {
    pub played: u64,
    /// Pairs left after dropping exact repeats (both games identical in every recorded number). All inference
    /// below uses these: a game the seed did not change is not new evidence.
    pub distinct: u64,
    pub a_better: u64,
    pub equal: u64,
    pub b_better: u64,
    /// Exact two-sided sign test on the non-tied distinct pairs.
    pub sign_p: f64,
    /// Mean score of A per pair with a t interval over distinct pairs.
    pub score: Est,
    pub elo: Est,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoundStats {
    pub a: i64,
    pub b: i64,
    /// Rounds won by A minus rounds won by B, per game, with a t interval over distinct pairs.
    pub diff_per_game: Est,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MapRow {
    pub map: String,
    pub games: u64,
    pub wins: u64,
    pub draws: u64,
    pub losses: u64,
    pub score: Est,
    pub round_diff_per_game: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SprtReport {
    pub elo0: f64,
    pub elo1: f64,
    pub alpha: f64,
    pub beta: f64,
    pub llr: f64,
    pub lower: f64,
    pub upper: f64,
    pub decision: SprtDecision,
    pub pairs_used: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LossRow {
    pub map: String,
    pub seed: u64,
    /// Slot A sat in when it lost: "A" or "B".
    pub slot: String,
    pub score: String,
    pub replay: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AbReport {
    pub schema: String,
    pub a: String,
    pub b: String,
    pub backend: String,
    pub ruleset_hash: String,
    pub maps: Vec<String>,
    pub seeds: Vec<u64>,
    pub games: u64,
    pub failed: u64,
    pub wins: u64,
    pub draws: u64,
    pub losses: u64,
    /// (W + D/2) / N with the Wilson interval over games (treats games as independent; see `pairs`).
    pub score: Est,
    pub pairs: PairStats,
    pub rounds: RoundStats,
    /// A's score when sitting in the team A slot (attacks first) and in the team B slot.
    pub score_as_team_a: f64,
    pub score_as_team_b: f64,
    pub per_map: Vec<MapRow>,
    pub sprt: Option<SprtReport>,
    /// "a_better", "b_better" or "inside_noise".
    pub verdict: String,
    pub verdict_text: String,
    pub warnings: Vec<String>,
    /// Games A lost (at most 20): read these replays before concluding anything.
    pub lost_games: Vec<LossRow>,
}

/// One usable pair, from A's point of view.
struct Pair<'a> {
    first: &'a GameResult,
    second: &'a GameResult,
    /// Mean of A's points in the two games: 0, 0.25, 0.5, 0.75 or 1.
    score: f64,
    round_diff_per_game: f64,
}

fn round_diff(r: &GameResult, slot: usize) -> f64 {
    (r.score[slot] - r.score[1 - slot]) as f64
}

/// t interval of a mean of scores in 0..=1, with the variance floored at `0.25 / n` so a handful of
/// identical results cannot produce a zero-width interval.
fn score_ci(scores: &[f64]) -> Est {
    let (mean, var) = mean_var(scores);
    let n = scores.len() as u64;
    if n < 2 {
        return Est::point(mean, n);
    }
    let half = t95(n - 1) * (var.max(0.25 / n as f64) / n as f64).sqrt();
    Est {
        value: mean,
        lo: Some(mean - half),
        hi: Some(mean + half),
        n,
    }
    .clamped(0.0, 1.0)
}

/// Fewest distinct pairs that can support a verdict: with fewer, even a clean sweep has a two-sided
/// sign-test p above 0.05.
pub const MIN_DISTINCT_PAIRS: u64 = 6;

fn verdict(a: &str, b: &str, pairs: &PairStats, rounds: &RoundStats) -> (String, String) {
    let s = &pairs.score;
    if s.lo.is_none() {
        let text = format!(
            "INSIDE THE NOISE: only {} distinct pair(s), which cannot measure the noise at all. Add maps or seeds.",
            pairs.distinct
        );
        return ("inside_noise".into(), text);
    }
    if s.excludes(0.5) && pairs.distinct < MIN_DISTINCT_PAIRS {
        let leader = if s.value > 0.5 { a } else { b };
        let text = format!(
            "NOT ESTABLISHED: {leader} is ahead (paired score of {a} is {}), but only {} distinct pairs were played and at least {MIN_DISTINCT_PAIRS} are needed before a difference counts. Add maps (repeating seeds adds nothing).",
            est_pct(s),
            pairs.distinct
        );
        return ("inside_noise".into(), text);
    }
    if s.excludes(0.5) {
        let (winner, key) = if s.value > 0.5 {
            (a, "a_better")
        } else {
            (b, "b_better")
        };
        let text = format!(
            "REAL DIFFERENCE: {winner} is better. Paired score of {a} is {} over {} distinct pairs (sign test p = {}).",
            est_pct(s),
            pairs.distinct,
            p_value(pairs.sign_p)
        );
        return (key.into(), text);
    }
    let mut text = format!(
        "INSIDE THE NOISE: paired score of {a} is {}; the interval contains 50%, so this run does not show that \
         either bot is better. Do not act on it.",
        est_pct(s)
    );
    if pairs.distinct >= 4 && rounds.diff_per_game.excludes(0.0) {
        let leader = if rounds.diff_per_game.value > 0.0 {
            a
        } else {
            b
        };
        text.push_str(&format!(
            " The round differential ({} rounds per game) does favour {leader}: it is the steadier signal, so \
             run more seeds before deciding.",
            est_num(&rounds.diff_per_game, 2)
        ));
    }
    ("inside_noise".into(), text)
}

/// Analyse results that are in `paired_schedule` order (games `2k`, `2k + 1` = pair `k`, A in slot A first).
pub fn analyse(
    a: &str,
    b: &str,
    results: &[GameResult],
    sprt: Option<Sprt>,
    mut warnings: Vec<String>,
) -> AbReport {
    let ok: Vec<&GameResult> = results.iter().filter(|r| r.ok()).collect();
    let slot_of_a = |r: &GameResult| if r.id % 2 == 0 { 0 } else { 1 };
    let points: Vec<f64> = ok.iter().map(|r| r.points(slot_of_a(r))).collect();
    let n = ok.len() as u64;
    let count = |p: f64| points.iter().filter(|&&x| x == p).count() as u64;
    let (wins, draws, losses) = (count(1.0), count(0.5), count(0.0));
    let score_value = if n == 0 {
        0.5
    } else {
        points.iter().sum::<f64>() / n as f64
    };
    let (lo, hi) = wilson(score_value, n as f64, Z95);
    let score = Est {
        value: score_value,
        lo: (n > 0).then_some(lo),
        hi: (n > 0).then_some(hi),
        n,
    };

    let pairs_all: Vec<Pair> = results
        .chunks(2)
        .filter(|c| c.len() == 2 && c[0].ok() && c[1].ok())
        .map(|c| Pair {
            first: &c[0],
            second: &c[1],
            score: (c[0].points(0) + c[1].points(1)) / 2.0,
            round_diff_per_game: (round_diff(&c[0], 0) + round_diff(&c[1], 1)) / 2.0,
        })
        .collect();
    let mut seen = BTreeSet::new();
    let distinct: Vec<&Pair> = pairs_all
        .iter()
        .filter(|p| seen.insert((p.first.signature(), p.second.signature())))
        .collect();
    let pair_scores: Vec<f64> = distinct.iter().map(|p| p.score).collect();
    let a_better = pair_scores.iter().filter(|&&s| s > 0.5).count() as u64;
    let b_better = pair_scores.iter().filter(|&&s| s < 0.5).count() as u64;
    let pair_score = score_ci(&pair_scores);
    let pairs = PairStats {
        played: pairs_all.len() as u64,
        distinct: distinct.len() as u64,
        a_better,
        equal: distinct.len() as u64 - a_better - b_better,
        b_better,
        sign_p: sign_test_p(a_better, a_better + b_better),
        score: pair_score,
        elo: Est {
            value: score_to_elo(pair_score.value),
            lo: pair_score.lo.map(score_to_elo),
            hi: pair_score.hi.map(score_to_elo),
            n: pair_score.n,
        },
    };
    let diffs: Vec<f64> = distinct.iter().map(|p| p.round_diff_per_game).collect();
    let rounds = RoundStats {
        a: ok.iter().map(|r| r.score[slot_of_a(r)] as i64).sum(),
        b: ok.iter().map(|r| r.score[1 - slot_of_a(r)] as i64).sum(),
        diff_per_game: crate::stats::mean_ci(&diffs),
    };

    let slot_score = |slot: usize| {
        let p: Vec<f64> = ok
            .iter()
            .filter(|r| slot_of_a(r) == slot)
            .map(|r| r.points(slot))
            .collect();
        if p.is_empty() {
            0.5
        } else {
            p.iter().sum::<f64>() / p.len() as f64
        }
    };
    let maps: Vec<String> = results
        .iter()
        .map(|r| r.map.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let per_map = maps
        .iter()
        .map(|map| {
            let games: Vec<&&GameResult> = ok.iter().filter(|r| &r.map == map).collect();
            let p: Vec<f64> = games.iter().map(|r| r.points(slot_of_a(r))).collect();
            let g = p.len() as u64;
            let c = |v: f64| p.iter().filter(|&&x| x == v).count() as u64;
            let s = if g == 0 {
                0.5
            } else {
                p.iter().sum::<f64>() / g as f64
            };
            let (lo, hi) = wilson(s, g as f64, Z95);
            MapRow {
                map: map.clone(),
                games: g,
                wins: c(1.0),
                draws: c(0.5),
                losses: c(0.0),
                score: Est {
                    value: s,
                    lo: (g > 0).then_some(lo),
                    hi: (g > 0).then_some(hi),
                    n: g,
                },
                round_diff_per_game: games
                    .iter()
                    .map(|r| round_diff(r, slot_of_a(r)))
                    .sum::<f64>()
                    / g.max(1) as f64,
            }
        })
        .collect();

    let sprt = sprt.map(|s| {
        let llr = s.llr(&pair_scores);
        let (lower, upper) = s.bounds();
        SprtReport {
            elo0: s.elo0,
            elo1: s.elo1,
            alpha: s.alpha,
            beta: s.beta,
            llr,
            lower,
            upper,
            decision: s.decide(llr),
            pairs_used: pair_scores.len() as u64,
        }
    });

    warnings.extend(crate::model::fuel_warnings(results));
    let failed = results.len() as u64 - n;
    if failed > 0 {
        warnings.push(format!("{failed} game(s) failed and are excluded; their pairs are dropped from the paired analysis"));
    }
    if pairs.distinct < pairs.played {
        warnings.push(format!(
            "{} of {} pairs are exact repeats of an earlier pair (same map, same game digest): the seed \
             did not change those games. Intervals and tests use the {} distinct pairs; more seeds will not help, \
             more maps will",
            pairs.played - pairs.distinct,
            pairs.played,
            pairs.distinct
        ));
    }
    let (verdict, verdict_text) = verdict(a, b, &pairs, &rounds);
    let mut seeds: Vec<u64> = Vec::new();
    for r in results {
        if !seeds.contains(&r.seed) {
            seeds.push(r.seed);
        }
    }
    AbReport {
        schema: SCHEMA.into(),
        a: a.into(),
        b: b.into(),
        backend: results
            .first()
            .map(|r| r.backend.clone())
            .unwrap_or_default(),
        ruleset_hash: ok
            .first()
            .map(|r| r.ruleset_hash.clone())
            .unwrap_or_default(),
        maps,
        seeds,
        games: n,
        failed,
        wins,
        draws,
        losses,
        score,
        pairs,
        rounds,
        score_as_team_a: slot_score(0),
        score_as_team_b: slot_score(1),
        per_map,
        sprt,
        verdict,
        verdict_text,
        warnings,
        lost_games: ok
            .iter()
            .filter(|r| r.points(slot_of_a(r)) == 0.0)
            .take(20)
            .map(|r| LossRow {
                map: r.map.clone(),
                seed: r.seed,
                slot: if slot_of_a(r) == 0 { "A" } else { "B" }.into(),
                score: format!("{}-{}", r.score[slot_of_a(r)], r.score[1 - slot_of_a(r)]),
                replay: r.replay.clone(),
            })
            .collect(),
    }
}

/// Play the paired schedule and analyse it. With `sprt`, pairs are played in batches of `batch_pairs`
/// in schedule order and the run stops at the first batch boundary where the test has decided (or where
/// a whole batch brought nothing but exact repeats of earlier pairs); the
/// stopping point depends only on the seed list and the batch size, never on thread timing.
pub fn run_ab(
    bots: &[Bot; 2],
    maps: &[MapEntry],
    seeds: &[u64],
    sprt: Option<Sprt>,
    batch_pairs: usize,
    opts: &RunOpts,
) -> Result<(AbReport, Vec<GameResult>), String> {
    let schedule = paired_schedule(maps.len(), seeds);
    let mut warnings = Vec::new();
    if bots[0].native == bots[1].native && bots[0].path == bots[1].path {
        warnings.push("mirror match: both sides are the same bot. This measures side and slot bias, not strength".into());
    }
    let (a, b) = (bots[0].label.as_str(), bots[1].label.as_str());
    let chunk = if sprt.is_some() {
        batch_pairs.max(1) * 2
    } else {
        schedule.len().max(1)
    };
    let mut results: Vec<GameResult> = Vec::new();
    let mut distinct_before = 0;
    for batch in schedule.chunks(chunk) {
        let run = run_games(bots, maps, batch, opts)?;
        results.extend(run.results);
        for w in run.warnings {
            if !warnings.contains(&w) {
                warnings.push(w);
            }
        }
        let so_far = analyse(a, b, &results, sprt, Vec::new());
        if so_far
            .sprt
            .is_some_and(|s| s.decision != SprtDecision::Continue)
        {
            break;
        }
        // A batch of nothing but exact repeats means further seeds only burn compute.
        let exhausted = sprt.is_some()
            && so_far.pairs.distinct == distinct_before
            && results.len() < schedule.len();
        if exhausted {
            warnings.push(format!(
                "stopped after {} of {} pairs: the last batch added no new distinct pair, so more seeds cannot move the test. Add maps to get more evidence",
                so_far.pairs.played,
                schedule.len() / 2
            ));
            break;
        }
        distinct_before = so_far.pairs.distinct;
    }
    Ok((analyse(a, b, &results, sprt, warnings), results))
}

pub fn render_markdown(r: &AbReport) -> String {
    let mut s = format!("# A/B: {} vs {}\n\n**{}**\n\n", r.a, r.b, r.verdict_text);
    for w in &r.warnings {
        s.push_str(&format!("> WARNING: {w}\n\n"));
    }
    s.push_str(&format!(
        "- backend: {}; ruleset `{}`, maps: {}, seeds: {} ({} games, {} failed)\n",
        backend_label(&r.backend),
        r.ruleset_hash,
        r.maps.join(", "),
        r.seeds.len(),
        r.games,
        r.failed
    ));
    s.push_str(&format!(
        "- {}: **{} wins, {} draws, {} losses**; score {} (Wilson over games)\n",
        r.a,
        r.wins,
        r.draws,
        r.losses,
        est_pct(&r.score)
    ));
    let p = &r.pairs;
    s.push_str(&format!(
        "- paired: {} pairs played, {} distinct; {} better in {}, level in {}, {} better in {}; sign test p = {:.4}\n",
        p.played, p.distinct, r.a, p.a_better, p.equal, r.b, p.b_better, p_value(p.sign_p)
    ));
    s.push_str(&format!(
        "- paired score {} = Elo difference {}\n",
        est_pct(&p.score),
        est_num(&p.elo, 0)
    ));
    s.push_str(&format!(
        "- rounds: {} to {}; differential {} rounds per game (paired)\n",
        r.rounds.a,
        r.rounds.b,
        est_num(&r.rounds.diff_per_game, 2)
    ));
    s.push_str(&format!(
        "- slot effect: {} scores {} in the team A slot (attacks first) and {} in the team B slot\n",
        r.a,
        pct(r.score_as_team_a),
        pct(r.score_as_team_b)
    ));
    if let Some(t) = &r.sprt {
        let what = match t.decision {
            SprtDecision::AcceptH1 => format!("accepted H1 (Elo difference >= {})", t.elo1),
            SprtDecision::AcceptH0 => format!("accepted H0 (Elo difference <= {})", t.elo0),
            SprtDecision::Continue => {
                "undecided: the seed list ran out before a bound was reached".to_string()
            }
        };
        s.push_str(&format!(
            "- SPRT elo0={} elo1={} alpha={} beta={}: LLR {:.3} in [{:.3}, {:.3}] after {} distinct pairs: **{what}**\n",
            t.elo0, t.elo1, t.alpha, t.beta, t.llr, t.lower, t.upper, t.pairs_used
        ));
    }
    s.push_str("\n## Per map\n\n");
    let rows: Vec<Vec<String>> = r
        .per_map
        .iter()
        .map(|m| {
            vec![
                m.map.clone(),
                m.games.to_string(),
                format!("{}-{}-{}", m.wins, m.draws, m.losses),
                est_pct(&m.score),
                format!("{:+.2}", m.round_diff_per_game),
            ]
        })
        .collect();
    s.push_str(&md_table(
        &[
            "map",
            "games",
            "W-D-L",
            "score (Wilson)",
            "round diff / game",
        ],
        &rows,
    ));
    if !r.lost_games.is_empty() {
        s.push_str(&format!(
            "\n## Games {} lost (a loss might be a bug: read the replay)\n\n",
            r.a
        ));
        let rows: Vec<Vec<String>> = r
            .lost_games
            .iter()
            .map(|l| {
                let replay = l
                    .replay
                    .clone()
                    .unwrap_or_else(|| "native: re-run with `ibc-bench record`".into());
                vec![
                    l.map.clone(),
                    l.seed.to_string(),
                    l.slot.clone(),
                    l.score.clone(),
                    replay,
                ]
            })
            .collect();
        s.push_str(&md_table(
            &["map", "seed", "slot", "rounds", "replay"],
            &rows,
        ));
    }
    s.push_str(
        "\nIntervals are 95 %. The paired interval is the one to trust: games on the same map and seed share \
         most of their variance, and exact repeats are counted once.\n",
    );
    s
}

/// The one-screen summary printed on stdout.
pub fn render_summary(r: &AbReport) -> String {
    let mut s = format!(
        "{} vs {}: +{} ={} -{} of {} games, score {}\n",
        r.a,
        r.b,
        r.wins,
        r.draws,
        r.losses,
        r.games,
        est_pct(&r.score)
    );
    s.push_str(&format!(
        "backend {}, ruleset {}\n",
        backend_label(&r.backend),
        r.ruleset_hash
    ));
    s.push_str(&format!(
        "paired: {} distinct pairs ({} played), {}/{}/{} better/level/worse, sign p = {}, score {}, Elo {}\n",
        r.pairs.distinct,
        r.pairs.played,
        r.pairs.a_better,
        r.pairs.equal,
        r.pairs.b_better,
        p_value(r.pairs.sign_p),
        est_pct(&r.pairs.score),
        est_num(&r.pairs.elo, 0)
    ));
    s.push_str(&format!(
        "rounds {}-{}, differential {} per game\n",
        r.rounds.a,
        r.rounds.b,
        est_num(&r.rounds.diff_per_game, 2)
    ));
    if let Some(t) = &r.sprt {
        s.push_str(&format!(
            "SPRT: LLR {:.3} in [{:.3}, {:.3}] after {} pairs -> {:?}\n",
            t.llr, t.lower, t.upper, t.pairs_used, t.decision
        ));
    }
    for w in &r.warnings {
        s.push_str(&format!("WARNING: {w}\n"));
    }
    s.push_str(&r.verdict_text);
    s.push('\n');
    s
}
