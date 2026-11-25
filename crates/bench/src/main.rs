//! `ibc-bench`: paired A/B tests, leagues, balance reports, noise tables and replay inspection.

use ibc_bench::backend::{default_jobs, record_native, BackendKind, RunOpts};
use ibc_bench::model::{load_maps, write_games_jsonl, Bot, GameResult, MapEntry};
use ibc_bench::report::{stamp, to_json, write_file};
use ibc_bench::stats::Sprt;
use ibc_bench::{ab, balance, compare, dump, league, noise};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage:
  ibc-bench ab <A> <B>      [--seeds K] [--sprt elo0=0 elo1=20 alpha=0.05 beta=0.05] [--batch PAIRS] [common]
  ibc-bench league          [--bots a,b,c] [--seeds K] [--mirrors] [common]
  ibc-bench balance         [--bots a,b,c] [--seeds K] [--no-mirrors] [--from games.jsonl] [common]
  ibc-bench balance --compare OLD/report.json NEW/report.json
  ibc-bench noise <A> <B>   [--seeds K] [common]
  ibc-bench record <A> <B> --map <file.ibcmap> [--seed N] [--out replay.ibcr]     (built-in bots, native)
  ibc-bench dump <replay.ibcr> [--out DIR]
  ibc-bench why <replay.ibcr> --round R --unit U [--all-ticks]

A bot is a built-in name (see `ibc-sim`), a bot directory or a .wasm file.
common: --maps <dir|file> (default maps)   --seed-list 3,7,9 (instead of --seeds: seeds 1..K)
        --backend auto|native|wasm (auto: native when both bots are built in)
        --jobs N   --out DIR (default out/<command>/<utc stamp>)   --max-ticks N
        --ibc <path to ibc> (or $IBC_BIN)   --bots-dir DIR (default bots)   --discard-replays
exit code: 0 ok, 1 error, 2 finished but some games failed";

/// Flags that take no value, and flags that take every following token up to the next `--flag`.
const SWITCHES: [&str; 4] = ["all-ticks", "discard-replays", "mirrors", "no-mirrors"];
const GREEDY: [&str; 3] = ["sprt", "bots", "compare"];

const KNOWN: [&str; 22] = [
    "all-ticks",
    "discard-replays",
    "mirrors",
    "no-mirrors",
    "sprt",
    "bots",
    "compare",
    "seeds",
    "seed-list",
    "maps",
    "backend",
    "jobs",
    "out",
    "max-ticks",
    "ibc",
    "bots-dir",
    "batch",
    "from",
    "map",
    "seed",
    "round",
    "unit",
];

struct Args {
    positional: Vec<String>,
    flags: BTreeMap<String, Vec<String>>,
}

impl Args {
    fn parse(args: &[String]) -> Result<Args, String> {
        let mut out = Args {
            positional: Vec::new(),
            flags: BTreeMap::new(),
        };
        let mut i = 0;
        while i < args.len() {
            let Some(name) = args[i].strip_prefix("--") else {
                out.positional.push(args[i].clone());
                i += 1;
                continue;
            };
            if !KNOWN.contains(&name) {
                return Err(format!(
                    "unknown flag --{name}
{USAGE}"
                ));
            }
            i += 1;
            let mut values = Vec::new();
            if GREEDY.contains(&name) {
                while i < args.len() && !args[i].starts_with("--") {
                    values.push(args[i].clone());
                    i += 1;
                }
            } else if !SWITCHES.contains(&name) {
                values.push(
                    args.get(i)
                        .cloned()
                        .ok_or_else(|| format!("--{name} needs a value"))?,
                );
                i += 1;
            }
            out.flags.insert(name.to_string(), values);
        }
        Ok(out)
    }

    fn has(&self, name: &str) -> bool {
        self.flags.contains_key(name)
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.flags
            .get(name)
            .and_then(|v| v.first())
            .map(String::as_str)
    }

    fn num<T: std::str::FromStr>(&self, name: &str, default: T) -> Result<T, String> {
        match self.get(name) {
            Some(v) => v
                .parse()
                .map_err(|_| format!("--{name}: `{v}` is not a valid number")),
            None => Ok(default),
        }
    }

    fn seeds(&self, default: u64) -> Result<Vec<u64>, String> {
        if let Some(list) = self.get("seed-list") {
            let seeds: Result<Vec<u64>, _> =
                list.split(',').map(|s| s.trim().parse::<u64>()).collect();
            return seeds.map_err(|_| {
                format!("--seed-list: `{list}` is not a comma-separated list of seeds")
            });
        }
        let k: u64 = self.num("seeds", default)?;
        if k == 0 {
            return Err("--seeds must be at least 1".into());
        }
        Ok((1..=k).collect())
    }

    fn bots_dir(&self) -> PathBuf {
        PathBuf::from(self.get("bots-dir").unwrap_or("bots"))
    }

    fn maps(&self) -> Result<Vec<MapEntry>, String> {
        load_maps(Path::new(self.get("maps").unwrap_or("maps")))
    }

    fn out_dir(&self, command: &str) -> PathBuf {
        self.get("out")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new("out").join(command).join(stamp()))
    }

    fn run_opts(&self, command: &str) -> Result<RunOpts, String> {
        let mut opts = RunOpts::new(self.out_dir(command));
        opts.backend = BackendKind::parse(self.get("backend").unwrap_or("auto"))?;
        opts.jobs = self.num("jobs", default_jobs())?.max(1);
        opts.ibc = self.get("ibc").map(PathBuf::from);
        opts.keep_replays = !self.has("discard-replays");
        opts.max_ticks = self.num("max-ticks", opts.max_ticks)?;
        Ok(opts)
    }

    /// `--bots a,b c` or the default pool.
    fn pool(&self, default: Vec<String>) -> Result<Vec<Bot>, String> {
        let specs: Vec<String> = match self.flags.get("bots") {
            Some(values) => values
                .iter()
                .flat_map(|v| v.split(','))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect(),
            None => default,
        };
        Bot::parse_pool(&specs, &self.bots_dir())
    }

    fn two_bots(&self) -> Result<[Bot; 2], String> {
        match self.positional.as_slice() {
            [a, b] => {
                let pool = Bot::parse_pool(&[a.clone(), b.clone()], &self.bots_dir())?;
                Ok([pool[0].clone(), pool[1].clone()])
            }
            _ => Err(format!("expected two bots\n{USAGE}")),
        }
    }
}

fn parse_sprt(tokens: &[String]) -> Result<Sprt, String> {
    let mut s = Sprt {
        elo0: 0.0,
        elo1: 20.0,
        alpha: 0.05,
        beta: 0.05,
    };
    for t in tokens {
        let (key, value) = t
            .split_once('=')
            .ok_or_else(|| format!("--sprt: expected key=value, got `{t}`"))?;
        let v: f64 = value
            .parse()
            .map_err(|_| format!("--sprt: `{value}` is not a number"))?;
        match key {
            "elo0" => s.elo0 = v,
            "elo1" => s.elo1 = v,
            "alpha" => s.alpha = v,
            "beta" => s.beta = v,
            other => {
                return Err(format!(
                    "--sprt: unknown key `{other}` (elo0, elo1, alpha, beta)"
                ))
            }
        }
    }
    let unit = |p: f64| p > 0.0 && p < 0.5;
    if s.elo1 <= s.elo0 || !unit(s.alpha) || !unit(s.beta) {
        return Err("--sprt needs elo1 > elo0 and alpha, beta in (0, 0.5)".into());
    }
    Ok(s)
}

/// 0, or 2 when some games failed: the reports are still written, but a script must not miss it.
fn status(results: &[GameResult]) -> u8 {
    let failed: Vec<&GameResult> = results.iter().filter(|r| !r.ok()).collect();
    for r in failed.iter().take(10) {
        eprintln!(
            "FAILED: {} vs {} on {} seed {}: {}",
            r.a,
            r.b,
            r.map,
            r.seed,
            r.error.as_deref().unwrap_or("")
        );
    }
    if failed.is_empty() {
        0
    } else {
        2
    }
}

fn cmd_ab(args: &Args) -> Result<u8, String> {
    let bots = args.two_bots()?;
    let (maps, seeds, opts) = (args.maps()?, args.seeds(10)?, args.run_opts("ab")?);
    let sprt = args.flags.get("sprt").map(|t| parse_sprt(t)).transpose()?;
    let (report, results) = ab::run_ab(
        &bots,
        &maps,
        &seeds,
        sprt,
        args.num("batch", 8usize)?,
        &opts,
    )?;
    write_file(&opts.out_dir.join("report.json"), &to_json(&report)?)?;
    write_file(
        &opts.out_dir.join("report.md"),
        &ab::render_markdown(&report),
    )?;
    write_games_jsonl(&opts.out_dir.join("games.jsonl"), &results)?;
    print!("{}", ab::render_summary(&report));
    println!("report: {}", opts.out_dir.join("report.md").display());
    Ok(status(&results))
}

fn cmd_league(args: &Args) -> Result<u8, String> {
    let bots = args.pool(ibc_sim::BOTS.iter().map(|s| s.to_string()).collect())?;
    let (maps, seeds, opts) = (args.maps()?, args.seeds(3)?, args.run_opts("league")?);
    let run = league::run_league(&bots, &maps, &seeds, args.has("mirrors"), &opts)?;
    let labels: Vec<String> = bots.iter().map(|b| b.label.clone()).collect();
    let report = league::table(&labels, &run.results, run.warnings);
    write_file(&opts.out_dir.join("report.json"), &to_json(&report)?)?;
    let text = league::render_markdown(&report);
    write_file(&opts.out_dir.join("report.md"), &text)?;
    write_games_jsonl(&opts.out_dir.join("games.jsonl"), &run.results)?;
    print!("{text}");
    println!("\nreport: {}", opts.out_dir.join("report.md").display());
    Ok(status(&run.results))
}

fn load_balance(path: &str) -> Result<balance::BalanceReport, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{path}: not a balance report.json ({e})"))
}

fn cmd_balance(args: &Args) -> Result<u8, String> {
    if let Some(files) = args.flags.get("compare") {
        let [old, new] = files.as_slice() else {
            return Err("--compare needs OLD.json NEW.json".into());
        };
        print!(
            "{}",
            compare::render(&compare::compare(&load_balance(old)?, &load_balance(new)?))
        );
        return Ok(0);
    }
    let mut opts = args.run_opts("balance")?;
    opts.detail = true;
    let (results, warnings) = match args.get("from") {
        Some(path) => (
            ibc_bench::model::read_games_jsonl(Path::new(path))?,
            Vec::new(),
        ),
        None => {
            // The do-nothing bot only produces timeouts; it is a sanity check, not part of the game's balance.
            let default = ibc_sim::BOTS
                .iter()
                .filter(|b| **b != "idle")
                .map(|s| s.to_string())
                .collect();
            let bots = args.pool(default)?;
            let run = league::run_league(
                &bots,
                &args.maps()?,
                &args.seeds(5)?,
                !args.has("no-mirrors"),
                &opts,
            )?;
            (run.results, run.warnings)
        }
    };
    let report = balance::build(&results, warnings);
    write_file(&opts.out_dir.join("report.json"), &to_json(&report)?)?;
    write_file(
        &opts.out_dir.join("report.md"),
        &balance::render_markdown(&report),
    )?;
    if !args.has("from") {
        write_games_jsonl(&opts.out_dir.join("games.jsonl"), &results)?;
    }
    print!("{}", balance::render_summary(&report));
    println!("report: {}", opts.out_dir.join("report.md").display());
    println!(
        "compare: ibc-bench balance --compare <old>/report.json {}",
        opts.out_dir.join("report.json").display()
    );
    Ok(status(&results))
}

fn cmd_noise(args: &Args) -> Result<u8, String> {
    let bots = args.two_bots()?;
    let (maps, seeds, opts) = (args.maps()?, args.seeds(40)?, args.run_opts("noise")?);
    let schedule = ibc_bench::model::paired_schedule(maps.len(), &seeds);
    let run = ibc_bench::backend::run_games(&bots, &maps, &schedule, &opts)?;
    let report = noise::build(&bots[0].label, &bots[1].label, &run.results, run.warnings);
    write_file(&opts.out_dir.join("report.json"), &to_json(&report)?)?;
    write_file(
        &opts.out_dir.join("report.md"),
        &noise::render(&report, true),
    )?;
    print!("{}", noise::render(&report, false));
    println!("report: {}", opts.out_dir.join("report.md").display());
    Ok(status(&run.results))
}

fn cmd_record(args: &Args) -> Result<u8, String> {
    let [a, b] = args.positional.as_slice() else {
        return Err(format!("expected two built-in bots\n{USAGE}"));
    };
    let map_path = args
        .get("map")
        .ok_or_else(|| format!("missing --map\n{USAGE}"))?;
    let map_text = std::fs::read_to_string(map_path).map_err(|e| format!("{map_path}: {e}"))?;
    let seed: u64 = args.num("seed", 1)?;
    let replay = record_native(
        &map_text,
        seed,
        a,
        b,
        args.num("max-ticks", ibc_sim::DEFAULT_MAX_TICKS)?,
    )?;
    let out = args.get("out").map(PathBuf::from).unwrap_or_else(|| {
        Path::new("out")
            .join("record")
            .join(format!("{a}-{b}-{seed}.ibcr"))
    });
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&out, replay.encode()).map_err(|e| format!("{}: {e}", out.display()))?;
    let r = &replay.header.result;
    println!(
        "winner={:?} score={}-{} ticks={} replay={}",
        r.winner,
        r.score[0],
        r.score[1],
        r.ticks,
        out.display()
    );
    Ok(0)
}

fn replay_arg(args: &Args) -> Result<(PathBuf, ibc_replay::Replay), String> {
    let [file] = args.positional.as_slice() else {
        return Err(format!("expected one replay file\n{USAGE}"));
    };
    Ok((PathBuf::from(file), dump::load_replay(Path::new(file))?))
}

fn cmd_dump(args: &Args) -> Result<u8, String> {
    let (file, replay) = replay_arg(args)?;
    let stem = file
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let dir = args
        .get("out")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new("out").join("dump").join(stem));
    let d = dump::dump(&replay)?;
    write_file(&dir.join("ticks.jsonl"), &d.ticks_jsonl)?;
    write_file(&dir.join("rounds.csv"), &d.rounds_csv)?;
    println!(
        "{}\n{}",
        dir.join("ticks.jsonl").display(),
        dir.join("rounds.csv").display()
    );
    Ok(0)
}

fn cmd_why(args: &Args) -> Result<u8, String> {
    let (_, replay) = replay_arg(args)?;
    let need = |name: &str| {
        args.get(name)
            .ok_or_else(|| format!("missing --{name}\n{USAGE}"))
    };
    let round: i32 = need("round")?
        .parse()
        .map_err(|_| "--round is not a number".to_string())?;
    let unit: u32 = need("unit")?
        .parse()
        .map_err(|_| "--unit is not a number".to_string())?;
    print!(
        "{}",
        dump::why(&replay, round, unit, args.has("all-ticks"))?
    );
    Ok(0)
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let run = |f: fn(&Args) -> Result<u8, String>| Args::parse(&argv[1..]).and_then(|a| f(&a));
    let result = match argv.first().map(String::as_str) {
        Some("ab") => run(cmd_ab),
        Some("league") => run(cmd_league),
        Some("balance") => run(cmd_balance),
        Some("noise") => run(cmd_noise),
        Some("record") => run(cmd_record),
        Some("dump") => run(cmd_dump),
        Some("why") => run(cmd_why),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
