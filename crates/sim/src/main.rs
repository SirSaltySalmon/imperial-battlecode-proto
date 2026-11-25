//! `ibc-sim run --a rusher --b holder --map maps/x.ibcmap --seed 1 [--frames out.jsonl]`
//! `ibc-sim league --maps maps --seeds 3 [--bots idle,rusher,holder]`

use ibc_sim::{attacker_rounds, run, run_with, SimResult, BOTS, DEFAULT_MAX_TICKS};
use std::collections::HashMap;
use std::io::Write;
use std::process::ExitCode;

const USAGE: &str = "usage:
  ibc-sim run --a <bot> --b <bot> --map <file.ibcmap> [--seed N] [--frames out.jsonl] [--max-ticks N]
  ibc-sim league --maps <dir> [--seeds N] [--bots a,b,c] [--max-ticks N]
bots: idle, rusher, holder";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("run") => parse_flags(&args[1..]).and_then(|f| cmd_run(&f)),
        Some("league") => parse_flags(&args[1..]).and_then(|f| cmd_league(&f)),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn parse_flags(args: &[String]) -> Result<HashMap<String, String>, String> {
    let mut flags = HashMap::new();
    let mut it = args.iter();
    while let Some(key) = it.next() {
        let name = key.strip_prefix("--").ok_or_else(|| format!("unexpected argument '{key}'\n{USAGE}"))?;
        let value = it.next().ok_or_else(|| format!("--{name} needs a value"))?;
        flags.insert(name.to_string(), value.clone());
    }
    Ok(flags)
}

fn num<T: std::str::FromStr>(flags: &HashMap<String, String>, key: &str, default: T) -> Result<T, String> {
    match flags.get(key) {
        Some(v) => v.parse().map_err(|_| format!("--{key}: '{v}' is not a valid number")),
        None => Ok(default),
    }
}

fn cmd_run(flags: &HashMap<String, String>) -> Result<(), String> {
    let get = |k: &str| flags.get(k).ok_or_else(|| format!("missing --{k}\n{USAGE}"));
    let (a, b, map_path) = (get("a")?, get("b")?, get("map")?);
    let seed: u64 = num(flags, "seed", 1)?;
    let max_ticks = num(flags, "max-ticks", DEFAULT_MAX_TICKS)?;
    let map_text = std::fs::read_to_string(map_path).map_err(|e| format!("{map_path}: {e}"))?;

    let r = match flags.get("frames") {
        Some(path) => {
            let file = std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?;
            let mut out = std::io::BufWriter::new(file);
            let mut io_error = None;
            let r = run_with(&map_text, seed, a, b, max_ticks, |m| {
                if let Err(e) = writeln!(out, "{}", m.frame_json()) {
                    io_error.get_or_insert(e);
                }
            })?;
            if let Some(e) = io_error.or_else(|| out.flush().err()) {
                return Err(format!("{path}: {e}"));
            }
            r
        }
        None => run(&map_text, seed, a, b, max_ticks)?,
    };
    println!("{}", r.summary_json);
    let winner = match r.winner {
        Some(0) => format!("A ({a})"),
        Some(1) => format!("B ({b})"),
        _ => "none".to_string(),
    };
    println!(
        "winner={winner} score={}-{} ticks={} hash={:016x} act_errors: A {}/{} B {}/{}",
        r.score[0], r.score[1], r.ticks, r.final_hash, r.acts[0].errors, r.acts[0].calls, r.acts[1].errors, r.acts[1].calls
    );
    if r.finished {
        Ok(())
    } else {
        Err(format!("match did not finish within {max_ticks} ticks"))
    }
}

#[derive(Default, Clone)]
struct Row {
    games: u32,
    wins: u32,
    ticks: u64,
    attack_won: u32,
    attack_played: u32,
    act_calls: u64,
    act_errors: u64,
}

fn cmd_league(flags: &HashMap<String, String>) -> Result<(), String> {
    let dir = flags.get("maps").ok_or_else(|| format!("missing --maps\n{USAGE}"))?;
    let seeds: u64 = num(flags, "seeds", 3)?;
    let max_ticks = num(flags, "max-ticks", DEFAULT_MAX_TICKS)?;
    let bots: Vec<String> = match flags.get("bots") {
        Some(list) => list.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        None => BOTS.iter().map(|s| s.to_string()).collect(),
    };
    let mut maps: Vec<(String, String)> = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{dir}: {e}"))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_some_and(|x| x == "ibcmap") {
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            maps.push((path.file_stem().unwrap_or_default().to_string_lossy().into_owned(), text));
        }
    }
    maps.sort();
    if maps.is_empty() || bots.len() < 2 || seeds == 0 {
        return Err(format!("league needs at least one map in {dir}, two bots and one seed"));
    }

    let mut rows: Vec<Row> = vec![Row::default(); bots.len()];
    // pair[a][b] = (wins of a as team A, games)
    let mut pairs = vec![vec![(0u32, 0u32); bots.len()]; bots.len()];
    let (mut games, mut failures) = (0u32, Vec::new());
    for (map_name, map_text) in &maps {
        for seed in 1..=seeds {
            for (ia, a) in bots.iter().enumerate() {
                for (ib, b) in bots.iter().enumerate() {
                    if ia == ib {
                        continue;
                    }
                    games += 1;
                    let label = format!("{a} vs {b} on {map_name} seed {seed}");
                    let outcome = std::panic::catch_unwind(|| run(map_text, seed, a, b, max_ticks));
                    let r: SimResult = match outcome {
                        Ok(Ok(r)) if r.finished => r,
                        Ok(Ok(r)) => {
                            failures.push(format!("{label}: not finished after {} ticks", r.ticks));
                            continue;
                        }
                        Ok(Err(e)) => {
                            failures.push(format!("{label}: {e}"));
                            continue;
                        }
                        Err(_) => {
                            failures.push(format!("{label}: panicked"));
                            continue;
                        }
                    };
                    let attack = attacker_rounds(&r.summary_json);
                    for (team, i) in [(0usize, ia), (1usize, ib)] {
                        let row = &mut rows[i];
                        row.games += 1;
                        row.wins += (r.winner == Some(team as i32)) as u32;
                        row.ticks += r.ticks as u64;
                        row.act_calls += r.acts[team].calls;
                        row.act_errors += r.acts[team].errors;
                    }
                    if let Some(attack) = attack {
                        for (team, i) in [(0usize, ia), (1usize, ib)] {
                            rows[i].attack_won += attack[team].0;
                            rows[i].attack_played += attack[team].1;
                        }
                    }
                    pairs[ia][ib].0 += (r.winner == Some(0)) as u32;
                    pairs[ia][ib].1 += 1;
                }
            }
        }
    }

    let pct = |n: u64, d: u64| if d == 0 { "   n/a".to_string() } else { format!("{:5.1}%", n as f64 * 100.0 / d as f64) };
    println!("maps: {}  seeds: {seeds}", maps.iter().map(|m| m.0.as_str()).collect::<Vec<_>>().join(", "));
    println!("{:<10} {:>5} {:>5} {:>7} {:>12} {:>10} {:>11}", "bot", "games", "wins", "win%", "atk-rounds%", "mean-ticks", "act-errors");
    for (name, row) in bots.iter().zip(&rows) {
        println!(
            "{:<10} {:>5} {:>5} {:>7} {:>12} {:>10} {:>11}",
            name,
            row.games,
            row.wins,
            pct(row.wins as u64, row.games as u64),
            pct(row.attack_won as u64, row.attack_played as u64),
            if row.games == 0 { 0 } else { row.ticks / row.games as u64 },
            pct(row.act_errors, row.act_calls),
        );
    }
    let (won, played) = rows.iter().fold((0u64, 0u64), |t, r| (t.0 + r.attack_won as u64, t.1 + r.attack_played as u64));
    println!("attacker side won {won} of {played} rounds ({})", pct(won, played).trim());
    println!("\nwins as team A (row) against team B (column):");
    print!("{:<10}", "");
    for b in &bots {
        print!(" {b:>8}");
    }
    println!();
    for (ia, a) in bots.iter().enumerate() {
        print!("{a:<10}");
        for ib in 0..bots.len() {
            let (w, g) = pairs[ia][ib];
            print!(" {:>8}", if ia == ib { "-".to_string() } else { format!("{w}/{g}") });
        }
        println!();
    }
    if failures.is_empty() {
        println!("LEAGUE OK games={games}");
        Ok(())
    } else {
        Err(format!("LEAGUE FAILED {}/{games} games:\n  {}", failures.len(), failures.join("\n  ")))
    }
}
