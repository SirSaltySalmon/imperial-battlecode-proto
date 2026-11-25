//! Markdown and one-screen text rendering of a balance report.

use crate::balance::{BalanceReport, BANDS};
use crate::report::{est_num, est_pct, md_table, pct};
use crate::stats::Est;

/// True for metrics that are fractions and read best as percentages.
pub fn is_fraction(key: &str) -> bool {
    key.ends_with("_rate")
        || key.ends_with(".share")
        || key.ends_with("_score")
        || key.contains("_rate.")
        || key.ends_with(".budget_used")
}

pub fn format_metric(key: &str, e: &Est) -> String {
    if is_fraction(key) {
        est_pct(e)
    } else {
        est_num(e, 2)
    }
}

fn metric_rows(r: &BalanceReport, prefix: &str) -> Vec<Vec<String>> {
    let rows = r.metrics.iter().filter(|(k, _)| k.starts_with(prefix));
    rows.map(|(k, e)| vec![format!("`{k}`"), format_metric(k, e), e.n.to_string()])
        .collect()
}

pub fn render_markdown(r: &BalanceReport) -> String {
    let mut s = format!(
        "# Balance report\n\n- ruleset `{}`, backend `{}`\n- bots: {}\n- maps: {}; {} seeds; {} games ({} distinct, {} failed), {} rounds\n\n",
        r.ruleset_hash,
        r.backend,
        r.bots.join(", "),
        r.maps.join(", "),
        r.seeds,
        r.games,
        r.distinct_games,
        r.failed,
        r.rounds
    );
    for w in &r.warnings {
        s.push_str(&format!("> WARNING: {w}\n\n"));
    }
    s.push_str("Values are `estimate [95 % interval]`; `n` is the number of independent games behind the interval.\n");
    let sections = [
        ("Sides", "attacker_round_win_rate"),
        ("How rounds end", "end_reason."),
        ("Objective", "plant_rate"),
        ("Pace", "round_ticks"),
        ("Pace: matches", "match_"),
        ("Pace: first contact", "first_"),
        ("Pace: quiet rounds", "no_"),
        ("Gunplay", "shots_per_"),
        ("Gunplay: damage", "damage_per_"),
        ("Gunplay: kills", "kills"),
        ("Gunplay: kill distance", "kill_distance"),
        ("Movement", "moved_"),
        ("Economy", "econ."),
        ("Match outcomes", "draw_rate"),
        ("Match outcomes: tie-break", "tiebreak_rate"),
        ("Match outcomes: slot", "team_a_slot_score"),
        ("Degenerate rounds", "degenerate."),
        ("Fuel", "fuel."),
    ];
    for (title, prefix) in sections {
        let rows = metric_rows(r, prefix);
        if !rows.is_empty() {
            s.push_str(&format!(
                "\n## {title}\n\n{}",
                md_table(&["metric", "value", "n"], &rows)
            ));
        }
    }
    s.push_str("\n## Attacker round wins by pairing\n\n");
    let rows: Vec<Vec<String>> = r
        .attack_matrix
        .iter()
        .map(|m| {
            let rate = m.attacker_wins as f64 / m.rounds.max(1) as f64;
            vec![
                m.attacker.clone(),
                m.defender.clone(),
                format!("{}/{}", m.attacker_wins, m.rounds),
                pct(rate),
            ]
        })
        .collect();
    s.push_str(&md_table(
        &["attacking bot", "defending bot", "rounds won", "rate"],
        &rows,
    ));
    s.push_str("\n## Round length (ticks, buy phase included)\n\n");
    let most = r
        .round_length_histogram
        .iter()
        .map(|b| b.rounds)
        .max()
        .unwrap_or(1)
        .max(1);
    let rows: Vec<Vec<String>> = r
        .round_length_histogram
        .iter()
        .map(|b| {
            vec![
                format!("{}-{}", b.from, b.to),
                b.rounds.to_string(),
                "#".repeat((b.rounds * 40 / most) as usize),
            ]
        })
        .collect();
    s.push_str(&md_table(&["ticks", "rounds", ""], &rows));
    if !r.economy_by_round.is_empty() {
        s.push_str("\n## Credits by round (mean per unit at the first buy tick, and spent in the buy phase)\n\n");
        let rows: Vec<Vec<String>> = r
            .economy_by_round
            .iter()
            .map(|e| {
                vec![
                    e.round.to_string(),
                    e.rounds.to_string(),
                    format!("{:.0}", e.attack_credits),
                    format!("{:.0}", e.attack_spent),
                    format!("{:.0}", e.defend_credits),
                    format!("{:.0}", e.defend_spent),
                ]
            })
            .collect();
        s.push_str(&md_table(
            &[
                "round",
                "rounds seen",
                "atk credits",
                "atk spent",
                "def credits",
                "def spent",
            ],
            &rows,
        ));
        s.push_str("\n## Weapon in hand at action start\n\n");
        let rows: Vec<Vec<String>> = r
            .loadouts
            .iter()
            .flat_map(|(side, weapons)| {
                weapons
                    .iter()
                    .map(move |(w, share)| vec![side.clone(), w.clone(), pct(*share)])
            })
            .collect();
        s.push_str(&md_table(&["side", "weapon", "share of units"], &rows));
    }
    if !r.fuel.is_empty() {
        s.push_str("\n## Fuel per bot (wasm games)\n\n");
        let rows: Vec<Vec<String>> = r
            .fuel
            .iter()
            .map(|f| {
                vec![
                    f.bot.clone(),
                    f.games.to_string(),
                    format!("{:.0}", f.mean_per_turn_p50),
                    format!("{:.0}", f.mean_per_turn_p90),
                    format!("{:.0}", f.mean_per_turn_max),
                    f.max_unit_turn.to_string(),
                    pct(f.budget_used),
                    format!("{} of {}", f.out_of_fuel_turns, f.turns),
                ]
            })
            .collect();
        s.push_str(&md_table(
            &[
                "bot",
                "games",
                "fuel/turn p50",
                "p90",
                "max",
                "worst single turn",
                "budget used",
                "out-of-fuel turns",
            ],
            &rows,
        ));
    }
    s.push_str(&format!(
        "\n## Degenerate games\n\n- rounds with zero shots: {}\n- rounds with zero movement: {}\n",
        r.degenerate.zero_shot_rounds, r.degenerate.zero_move_rounds
    ));
    for line in &r.degenerate.always_timeout {
        s.push_str(&format!("- always times out: {line}\n"));
    }
    for line in &r.degenerate.never_shoot {
        s.push_str(&format!("- nobody ever shoots: {line}\n"));
    }
    s
}

/// The one-screen summary printed on stdout.
pub fn render_summary(r: &BalanceReport) -> String {
    let mut s = format!(
        "balance: ruleset {} | {} | {} games ({} distinct, {} failed), {} rounds | bots: {} | maps: {}\n",
        r.ruleset_hash,
        r.backend,
        r.games,
        r.distinct_games,
        r.failed,
        r.rounds,
        r.bots.join(", "),
        r.maps.join(", ")
    );
    let mut line = |label: &str, key: &str| {
        if let Some(e) = r.metrics.get(key) {
            s.push_str(&format!("  {label:<28} {}\n", format_metric(key, e)));
        }
    };
    line("attackers win rounds", "attacker_round_win_rate");
    for map in &r.maps {
        line(
            &format!("  on {map}"),
            &format!("attacker_round_win_rate.map.{map}"),
        );
    }
    for reason in ["elimination", "detonation", "defuse", "timeout"] {
        line(
            &format!("ends by {reason}"),
            &format!("end_reason.{reason}.share"),
        );
    }
    line("plant rate", "plant_rate");
    line("round length (ticks)", "round_ticks.mean");
    line("first contact (ticks)", "first_contact_ticks.mean");
    line("first kill (ticks)", "first_kill_ticks.mean");
    line("shots per kill", "shots_per_kill");
    line("damage per round", "damage_per_round");
    line("kill distance (tiles)", "kill_distance_tiles.mean");
    line("draws", "draw_rate");
    line("tie-breaks", "tiebreak_rate");
    line("zero-shot rounds", "degenerate.zero_shot_rounds.share");
    line("zero-move rounds", "degenerate.zero_move_rounds.share");
    let weapons: Vec<String> = r
        .metrics
        .iter()
        .filter_map(|(k, e)| {
            Some(format!(
                "{} {}",
                k.strip_prefix("kills.weapon.")?.strip_suffix(".share")?,
                pct(e.value)
            ))
        })
        .collect();
    if !weapons.is_empty() {
        s.push_str(&format!(
            "  {:<28} {}\n",
            "kills by weapon",
            weapons.join(", ")
        ));
    }
    let bands: Vec<String> = BANDS
        .iter()
        .filter_map(|(band, _)| {
            Some(format!(
                "{band}: {}",
                pct(r
                    .metrics
                    .get(&format!("kills.distance.{band}.share"))?
                    .value)
            ))
        })
        .collect();
    if !bands.is_empty() {
        s.push_str(&format!(
            "  {:<28} {}\n",
            "kills by distance (tiles)",
            bands.join(", ")
        ));
    }
    for f in &r.fuel {
        s.push_str(&format!(
            "  fuel {:<23} p50 {:.0}/turn, worst turn {}, {} of budget, {} out-of-fuel turns\n",
            f.bot,
            f.mean_per_turn_p50,
            f.max_unit_turn,
            pct(f.budget_used),
            f.out_of_fuel_turns
        ));
    }
    for w in &r.warnings {
        s.push_str(&format!("WARNING: {w}\n"));
    }
    s
}
