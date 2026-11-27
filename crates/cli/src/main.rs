//! `ibc`: the Imperial Battlecode command line.

mod admin;
mod batch;
mod build;
mod embedded;
mod init;
mod job;
mod remote;
mod new;
mod play;
mod replay_view;
mod toolchain;
mod update;
mod view;
mod view_library;
mod workspace;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use ibc_replay::{hash_hex, Replay};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "ibc", about = "Imperial Battlecode: build bots, run matches, inspect replays", disable_version_flag = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build a bot directory into <bot_dir>/.ibc/bot.wasm
    Build {
        bot_dir: PathBuf,
        /// Optimised build (the default; use --debug for an unoptimised one)
        #[arg(long, conflicts_with = "debug")]
        release: bool,
        /// Unoptimised build
        #[arg(long)]
        debug: bool,
    },
    /// Run one match between two bots (bot directories or .wasm files)
    Run {
        bot_a: PathBuf,
        /// Default: `[run] opponent` of the workspace's ibc.toml
        bot_b: Option<PathBuf>,
        /// Default: `[run] map` of the workspace's ibc.toml
        #[arg(long)]
        map: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        seed: u64,
        /// Output directory for replay.ibcr, summary.json and the team logs
        #[arg(short, long, default_value = "out/run")]
        out: PathBuf,
        /// Also write frames.jsonl (full state of every tick)
        #[arg(long)]
        frames: bool,
        /// Stop after this many ticks
        #[arg(long)]
        max_ticks: Option<u32>,
        /// Fuel multiplier of team A (default: the language default of the bot)
        #[arg(long)]
        fuel_mult_a: Option<f64>,
        /// Fuel multiplier of team B
        #[arg(long)]
        fuel_mult_b: Option<f64>,
    },
    /// Run many matches in parallel from a manifest, or from JSON jobs on stdin (--jsonl)
    RunBatch {
        /// Text manifest: `botA botB map seed outdir` per line (paths relative to the manifest)
        #[arg(required_unless_present = "jsonl", conflicts_with = "jsonl")]
        manifest: Option<PathBuf>,
        /// Read one JSON job per stdin line and write only JSON events to stdout (see docs/JOBS.md)
        #[arg(long)]
        jsonl: bool,
        /// Matches to run at once (default: number of cores)
        #[arg(long, short)]
        jobs: Option<usize>,
    },
    /// Inspect and verify replays
    Replay {
        #[command(subcommand)]
        command: ReplayCommand,
    },
    /// Print the patch identity: engine, ruleset hash, wasmtime and ABI versions
    Version {
        #[arg(long)]
        json: bool,
    },
    /// Manage the compilers bots are built with (wasi-sdk for C++ bots)
    Toolchain {
        #[command(subcommand)]
        command: ToolchainCommand,
    },
    /// Serve the replay viewer on localhost: ibc view [replay.ibcr] [--port N] [--viewer-dir DIR]
    View {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Moderation terminal for organisers: overview, flags, audit, suspend / ban teams, workers, dead letters
    Admin(admin::AdminArgs),
    #[command(flatten)]
    Remote(remote::RemoteCommand),
    /// Create a participant workspace (SDK, maps, docs, a starter bot), or refresh one after an update
    Init(init::InitArgs),
    /// Create bots/NAME in the current workspace from a template
    New(new::NewArgs),
    /// Ask the server which ibc it runs and print the command that upgrades this one (never downloads)
    Update {
        /// Server base URL (default: IBC_SERVER)
        #[arg(long)]
        server: Option<String>,
    },
    /// Play alongside your team's bot in the browser: ibc play TEAM_BOT OPPONENT [--map M] [--unit SLOT] ...
    Play {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum ToolchainCommand {
    /// Download the pinned wasi-sdk release, verify its sha256 and unpack it into the per-user cache
    Install {
        /// Reinstall even if it is already there
        #[arg(long)]
        force: bool,
        /// Use this already downloaded release archive instead of downloading (still verified)
        #[arg(long)]
        archive: Option<PathBuf>,
    },
    /// Show which wasi-sdk `ibc build` would use
    Status,
}

#[derive(Subcommand)]
enum ReplayCommand {
    /// Re-simulate and compare every state hash and the final score
    Verify { file: PathBuf },
    /// Re-simulate and print the full state of every tick as JSON lines
    Dump { file: PathBuf },
    /// Print the replay header as JSON
    Info { file: PathBuf },
    /// Download the replay of a platform match
    Download(remote::ReplayDownload),
    /// Write a copy without the other team's memory writes, logs, fuel and debug drawings
    Redact {
        input: PathBuf,
        output: PathBuf,
        /// Team whose private data stays: A, B or none
        #[arg(long, value_parser = replay_view::parse_keep)]
        keep: ibc_replay::Keep,
    },
    /// Print the private records (memory writes, logs, per-turn fuel, debug drawings) as JSON
    Records { file: PathBuf },
}

fn load_replay(file: &Path) -> Result<Replay> {
    let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    Replay::decode(&bytes).map_err(|e| anyhow!("{}: {e}", file.display()))
}

fn replay(command: ReplayCommand) -> Result<()> {
    match command {
        ReplayCommand::Verify { file } => {
            let r = load_replay(&file)?;
            ibc_replay::verify(&r).map_err(|e| anyhow!("{}: verification FAILED: {e}", file.display()))?;
            let res = &r.header.result;
            println!("ok: {} ticks, score {}-{}, every state hash matches", res.ticks, res.score[0], res.score[1]);
            for t in (0..2).filter(|&t| r.header.redacted[t]) {
                println!("(team {} is redacted: its memory was not checked)", ["A", "B"][t]);
            }
        }
        ReplayCommand::Dump { file } => {
            let r = load_replay(&file)?;
            let mut out = std::io::BufWriter::new(std::io::stdout().lock());
            let mut io_error = None;
            let sim = r.simulate(|m| {
                if io_error.is_none() {
                    io_error = writeln!(out, "{}", m.frame_json()).err();
                }
            });
            // A closed pipe (`| head`) is not an error worth reporting.
            if io_error.is_none() {
                io_error = out.flush().err();
            }
            match io_error {
                Some(e) if e.kind() != std::io::ErrorKind::BrokenPipe => return Err(e.into()),
                _ => {}
            }
            sim.map_err(|e| anyhow!("{}: {e}", file.display()))?;
        }
        ReplayCommand::Info { file } => {
            let r = load_replay(&file)?;
            println!("{}", serde_json::to_string_pretty(&r.header)?);
        }
        ReplayCommand::Download(args) => remote::replay_download(args)?,
        ReplayCommand::Redact { input, output, keep } => replay_view::redact(&input, &output, keep)?,
        ReplayCommand::Records { file } => replay_view::records(&file)?,
    }
    Ok(())
}

fn version(json: bool) {
    let ruleset = hash_hex(ibc_engine::Ruleset::builtin().hash());
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ibc": env!("CARGO_PKG_VERSION"),
                "engine": ibc_engine::ENGINE_VERSION,
                "ruleset_hash": ruleset,
                "wasmtime": ibc_runner::WASMTIME_VERSION,
                "abi": ibc_abi::ABI_VERSION,
                "replay_format": ibc_replay::FORMAT_VERSION,
            })
        );
    } else {
        println!("ibc {}", env!("CARGO_PKG_VERSION"));
        println!("engine {}", ibc_engine::ENGINE_VERSION);
        println!("ruleset {ruleset}");
        println!("wasmtime {}", ibc_runner::WASMTIME_VERSION);
        println!("abi {}", ibc_abi::ABI_VERSION);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Build { bot_dir, release: _, debug } => {
            let built = build::build_bot(&bot_dir, !debug)?;
            println!("{}", built.wasm_path.display());
            println!("sha256 {}", built.sha256);
            println!("language {}", built.language.name());
        }
        Command::Run { bot_a, bot_b, map, seed, out, frames, max_ticks, fuel_mult_a, fuel_mult_b } => {
            let (bot_b, map) = workspace::run_defaults(bot_b, map)?;
            let mut a = job::resolve_bot(&bot_a)?;
            let mut b = job::resolve_bot(&bot_b)?;
            a.fuel_multiplier = fuel_mult_a.unwrap_or(a.fuel_multiplier);
            b.fuel_multiplier = fuel_mult_b.unwrap_or(b.fuel_multiplier);
            let spec = job::JobSpec { bot_a: a, bot_b: b, map, seed, out_dir: out, max_ticks, frames };
            let r = job::execute(&spec)?;
            println!(
                "{} | seed {} | replay {} sha256 {}",
                job::result_line(&r.summary),
                seed,
                r.replay_path.display(),
                r.summary.replay_sha256
            );
        }
        Command::RunBatch { manifest, jsonl, jobs } => {
            let jobs = jobs.unwrap_or_else(batch::default_jobs).max(1);
            match (jsonl, manifest) {
                (true, _) => batch::run_jsonl(jobs)?,
                (false, Some(m)) => batch::run_manifest(&m, jobs)?,
                (false, None) => bail!("run-batch needs a manifest or --jsonl"),
            }
        }
        Command::Replay { command } => replay(command)?,
        Command::Version { json } => version(json),
        Command::Toolchain { command: ToolchainCommand::Install { force, archive } } => {
            toolchain::install(force, archive.as_deref())?
        }
        Command::Toolchain { command: ToolchainCommand::Status } => toolchain::status()?,
        Command::View { args } => view::run(args)?,
        Command::Admin(args) => admin::run(args)?,
        Command::Remote(command) => remote::run(command)?,
        Command::Init(args) => init::run(args)?,
        Command::New(args) => new::run(args)?,
        Command::Update { server } => update::run(server)?,
        Command::Play { args } => play::run(args)?,
    }
    Ok(())
}

fn main() {
    update::check_before(std::env::args_os().nth(1).as_deref().and_then(|a| a.to_str()));
    if let Err(e) = run(Cli::parse()) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
