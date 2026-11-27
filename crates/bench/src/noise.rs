//! `ibc-bench noise`: how noisy is a win rate between two bots, as a function of the number of games?
//! Run it once per pairing you care about; it tells you how many games a claim needs.

use crate::ab::{analyse, AbReport};
use crate::model::GameResult;
use crate::report::{backend_label, md_table, pct};
use crate::stats::{elo_to_score, mean_var, normal_quantile, Z95};
use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "ibc-bench/noise/1";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoiseRow {
    pub games: u64,
    /// Standard deviation of the score if the games were independent (no pairing).
    pub sd_unpaired: f64,
    /// Standard deviation of the score with the paired design (`games / 2` pairs).
    pub sd_paired: f64,
    /// Standard deviation of the score measured across disjoint chunks of this size; `None` with fewer
    /// than three chunks.
    pub sd_observed: Option<f64>,
    /// Half-width of the 95 % interval of the paired score: differences smaller than this are noise.
    pub half_width_95: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NeedRow {
    pub elo: f64,
    pub score_gap: f64,
    /// Games (two per pair) to detect this gap at 95 % confidence with 80 % power; `None` when the
    /// sample shows no variation.
    pub games: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoiseReport {
    pub schema: String,
    pub a: String,
    pub b: String,
    pub games: u64,
    pub pairs: u64,
    pub distinct_pairs: u64,
    pub score: f64,
    /// Standard deviation of one game's result and of one pair's mean result.
    pub sd_game: f64,
    pub sd_pair: f64,
    pub table: Vec<NoiseRow>,
    pub needed: Vec<NeedRow>,
    pub warnings: Vec<String>,
    pub ab: AbReport,
}

/// Results must be in `paired_schedule` order.
pub fn build(a: &str, b: &str, results: &[GameResult], warnings: Vec<String>) -> NoiseReport {
    let ab = analyse(a, b, results, None, warnings);
    let pairs: Vec<f64> = results
        .chunks(2)
        .filter(|c| c.len() == 2 && c[0].ok() && c[1].ok())
        .map(|c| (c[0].points(0) + c[1].points(1)) / 2.0)
        .collect();
    let games: Vec<f64> = results
        .iter()
        .filter(|r| r.ok())
        .map(|r| r.points(if r.id % 2 == 0 { 0 } else { 1 }))
        .collect();
    let (sd_game, sd_pair) = (mean_var(&games).1.sqrt(), mean_var(&pairs).1.sqrt());

    let mut sizes: Vec<usize> = std::iter::successors(Some(2usize), |n| Some(n * 2))
        .take_while(|&n| n < pairs.len() * 2)
        .collect();
    sizes.push(pairs.len() * 2);
    let table = sizes
        .into_iter()
        .filter(|&n| n >= 2)
        .map(|n| {
            let chunk_means: Vec<f64> = pairs
                .chunks_exact(n / 2)
                .map(|c| c.iter().sum::<f64>() / c.len() as f64)
                .collect();
            let sd_paired = sd_pair / ((n / 2) as f64).sqrt();
            NoiseRow {
                games: n as u64,
                sd_unpaired: sd_game / (n as f64).sqrt(),
                sd_paired,
                sd_observed: (chunk_means.len() >= 3).then(|| mean_var(&chunk_means).1.sqrt()),
                half_width_95: Z95 * sd_paired,
            }
        })
        .collect();
    // Two-sided 95 % test with 80 % power: n pairs = ((z_0.975 + z_0.80) sd / gap)^2.
    let z = Z95 + normal_quantile(0.80);
    let needed = [10.0, 20.0, 50.0, 100.0, 200.0]
        .into_iter()
        .map(|elo| {
            let gap = elo_to_score(elo) - 0.5;
            let games =
                (sd_pair > 0.0).then(|| 2 * ((z * sd_pair / gap).powi(2).ceil() as u64).max(2));
            NeedRow {
                elo,
                score_gap: gap,
                games,
            }
        })
        .collect();
    let mut warnings = ab.warnings.clone();
    if sd_pair == 0.0 && !pairs.is_empty() {
        warnings.push(
            "every pair had the same result: this pairing has no measurable noise on these maps and seeds. \
             The uncertainty that remains is whether other maps or opponents behave the same"
                .into(),
        );
    }
    NoiseReport {
        schema: SCHEMA.into(),
        a: a.into(),
        b: b.into(),
        games: games.len() as u64,
        pairs: pairs.len() as u64,
        distinct_pairs: ab.pairs.distinct,
        score: ab.score.value,
        sd_game,
        sd_pair,
        table,
        needed,
        warnings,
        ab,
    }
}

pub fn render(r: &NoiseReport, markdown: bool) -> String {
    let mut s = if markdown {
        format!("# Noise: {} vs {}\n\n", r.a, r.b)
    } else {
        format!("noise: {} vs {}\n", r.a, r.b)
    };
    s.push_str(&format!(
        "backend {}, ruleset {}\n",
        backend_label(&r.ab.backend),
        r.ab.ruleset_hash
    ));
    s.push_str(&format!(
        "{} games in {} pairs ({} distinct); score of {} = {}; sd of one game {:.3}, sd of one pair {:.3}\n\n",
        r.games,
        r.pairs,
        r.distinct_pairs,
        r.a,
        pct(r.score),
        r.sd_game,
        r.sd_pair
    ));
    let rows: Vec<Vec<String>> = r
        .table
        .iter()
        .map(|t| {
            vec![
                t.games.to_string(),
                pct(t.sd_unpaired),
                pct(t.sd_paired),
                t.sd_observed.map(pct).unwrap_or_else(|| "-".into()),
                format!("+/- {}", pct(t.half_width_95)),
            ]
        })
        .collect();
    let headers = [
        "games N",
        "sd of score, unpaired",
        "sd, paired",
        "sd, observed chunks",
        "95 % half-width (paired)",
    ];
    let needs: Vec<Vec<String>> = r
        .needed
        .iter()
        .map(|n| {
            let games = n
                .games
                .map(|g| g.to_string())
                .unwrap_or_else(|| "n/a".into());
            vec![
                format!("{:.0}", n.elo),
                format!("{:+.1} pts", n.score_gap * 100.0),
                games,
            ]
        })
        .collect();
    let need_headers = ["Elo gap", "score gap", "games needed (95 %, power 80 %)"];
    if markdown {
        s.push_str(&md_table(&headers, &rows));
        s.push_str("\n## Games needed to detect a gap\n\n");
        s.push_str(&md_table(&need_headers, &needs));
    } else {
        s.push_str(&format!(
            "{:>8} {:>22} {:>11} {:>20} {:>25}\n",
            headers[0], headers[1], headers[2], headers[3], headers[4]
        ));
        for row in &rows {
            s.push_str(&format!(
                "{:>8} {:>22} {:>11} {:>20} {:>25}\n",
                row[0], row[1], row[2], row[3], row[4]
            ));
        }
        s.push_str(&format!(
            "\n{:>8} {:>10} {}\n",
            need_headers[0], need_headers[1], need_headers[2]
        ));
        for row in &needs {
            s.push_str(&format!("{:>8} {:>10} {}\n", row[0], row[1], row[2]));
        }
    }
    s.push_str("\nA difference smaller than the half-width at your N is noise. The paired column is what `ab` achieves.\n");
    for w in &r.warnings {
        s.push_str(&format!(
            "{}WARNING: {w}\n",
            if markdown { "\n> " } else { "" }
        ));
    }
    s
}
