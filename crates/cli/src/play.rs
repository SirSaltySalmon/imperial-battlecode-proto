//! `ibc play OPPONENT [--map M] [--seed S] [--mode realtime|turn] [--tps N] [--team A|B] [--unit SLOT]
//! [--allies BOT] [--port N] [--out DIR] [--viewer-dir DIR]`: play one unit yourself against a bot, in the
//! browser.
//!
//! The match is an ordinary wasmtime match (`ibc_runner::run_driven`) in which the player's unit is driven
//! from the page instead of by a bot, through the same `act` calls and the same validation. Its teammates
//! run the `--allies` bot, or stand still. At the end the usual `replay.ibcr` and `summary.json` are written
//! to `--out`; the player's acts are in the replay like any bot's, so `ibc replay verify` re-simulates it.
//! See docs/PLAY.md.

mod input;
mod server;
mod session;
mod state;
#[cfg(test)]
mod tests;

use crate::job::{self, BotInput};
use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, ValueEnum};
use ibc_engine::{Map, Match, Ruleset};
use ibc_runner::{BotSpec, MatchConfig};
use serde_json::{json, Value};
use session::{Mode, Seat, Session};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const DEFAULT_MAP: &str = "maps/crossfire.ibcmap";

#[derive(Parser, Debug)]
#[command(name = "ibc play", about = "Play one unit against a bot in the browser")]
struct Args {
    /// The opposing bot (a bot directory or a .wasm file)
    opponent: PathBuf,
    /// Map file (default: maps/crossfire.ibcmap under the working directory)
    #[arg(long)]
    map: Option<PathBuf>,
    #[arg(long, default_value_t = 0)]
    seed: u64,
    /// realtime: ticks run on a clock (paused at the start); turn: one tick per step (Space)
    #[arg(long, value_enum, default_value_t = ModeArg::Realtime)]
    mode: ModeArg,
    /// Ticks per second in real-time mode
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=60))]
    tps: u32,
    /// Your team: A or B
    #[arg(long, default_value = "A", value_parser = parse_team)]
    team: i32,
    /// Which unit of your team you control (0-based slot)
    #[arg(long, default_value_t = 0)]
    unit: u32,
    /// Bot that runs your teammates (default: they stand still)
    #[arg(long)]
    allies: Option<PathBuf>,
    /// Port to listen on (default: any free port)
    #[arg(long, default_value_t = 0)]
    port: u16,
    /// Output directory for replay.ibcr, summary.json and the team logs
    #[arg(short, long, default_value = "out/play")]
    out: PathBuf,
    /// Directory with the built viewer (`viewer/dist`); also read from IBC_VIEWER_DIR
    #[arg(long)]
    viewer_dir: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ModeArg {
    Realtime,
    Turn,
}

fn parse_team(s: &str) -> Result<i32, String> {
    match s {
        "A" | "a" | "0" => Ok(0),
        "B" | "b" | "1" => Ok(1),
        _ => Err(format!("team must be A or B, not {s:?}")),
    }
}

/// Everything a session is made from, already read from disk.
pub struct Setup {
    pub map_text: String,
    pub seed: u64,
    pub mode: Mode,
    pub team: i32,
    pub slot: u32,
    pub opponent: BotSpec,
    pub allies: Option<BotSpec>,
}

pub fn run(args: Vec<String>) -> Result<()> {
    let args = Args::parse_from(std::iter::once("ibc play".to_string()).chain(args));
    let root = crate::view::find_viewer_dir(args.viewer_dir.as_deref())?;
    if !root.join("play.html").is_file() {
        bail!("{} has no play.html: rebuild the viewer (`cd viewer && npm run build`)", root.display());
    }
    let map = args.map.clone().unwrap_or_else(|| PathBuf::from(DEFAULT_MAP));
    let map_text = std::fs::read_to_string(&map)
        .with_context(|| format!("reading map {} (choose one with --map)", map.display()))?;
    let setup = Setup {
        map_text,
        seed: args.seed,
        mode: match args.mode {
            ModeArg::Realtime => Mode::Realtime { tps: args.tps },
            ModeArg::Turn => Mode::Turn,
        },
        team: args.team,
        slot: args.unit,
        opponent: bot_spec(&job::resolve_bot(&args.opponent)?)?,
        allies: args.allies.as_deref().map(|a| job::resolve_bot(a).and_then(|b| bot_spec(&b))).transpose()?,
    };
    let (cfg, session) = prepare(setup)?;

    let listener =
        TcpListener::bind(("127.0.0.1", args.port)).with_context(|| format!("listening on 127.0.0.1:{}", args.port))?;
    println!("http://127.0.0.1:{}/", listener.local_addr()?.port());
    eprintln!(
        "you are unit {} of team {}; serving {} (Ctrl-C to stop)",
        session.seat.unit,
        team_name(session.seat.team),
        root.display()
    );
    let site = Arc::new(server::Site { root, session: Arc::clone(&session) });
    std::thread::spawn(move || server::serve(listener, site));

    let result = ibc_runner::run_driven(cfg, &mut session.driver());
    let output = result.and_then(|out| finish(&session, &out, &args.out));
    if let Err(e) = &output {
        eprintln!("error: {e:#}");
        session.finish(json!({ "error": format!("{e:#}") }));
    }
    // Keep serving until Quit, so the page can show the result (or the error).
    if !session.quitting() {
        eprintln!("the match is over; the page shows the result. Stop with Ctrl-C or the page's Quit button");
        session.wait_quit();
    }
    output.map_err(|_| anyhow!("ibc play failed (the error is above)"))
}

fn team_name(team: i32) -> &'static str {
    if team == 0 {
        "A"
    } else {
        "B"
    }
}

fn bot_spec(b: &BotInput) -> Result<BotSpec> {
    b.spec()
}

/// The match config and the session for a setup. Checks the seat against the map's team size.
pub fn prepare(s: Setup) -> Result<(MatchConfig, Arc<Session>)> {
    let rules = Ruleset::builtin();
    let map = Map::parse(&s.map_text).map_err(|e| anyhow!("map: {e}"))?;
    let start = Match::new(rules.clone(), map, s.seed);
    let team_size = start.world.team_size as u32;
    if s.slot >= team_size {
        bail!("--unit {} does not exist: teams have {team_size} units (0..{})", s.slot, team_size - 1);
    }
    let seat = Seat { unit: s.team as u32 * team_size + s.slot, team: s.team, allies: s.allies.is_some() };
    // What the page sees until the match thread publishes tick 0 (the same view).
    let first_view = state::view(&start, seat.unit);
    let human = match s.allies {
        Some(a) => BotSpec { name: format!("human+{}", a.name), ..a },
        None => BotSpec::new("human", Vec::new()),
    };
    let setup_json = setup_json(&rules, &s.map_text, s.mode, seat, [&human.name, &s.opponent.name], s.team);
    let bots = if s.team == 0 { [human, s.opponent] } else { [s.opponent, human] };
    let cfg = MatchConfig {
        rules,
        map_text: s.map_text,
        seed: s.seed,
        bots,
        log_limit: ibc_runner::DEFAULT_LOG_LIMIT,
        max_ticks: None,
        frames: false,
    };
    Ok((cfg, Session::new(s.mode, seat, setup_json, first_view)))
}

/// What the page needs once: the map, the rules it draws with, names for ids, the seat and the mode.
fn setup_json(rules: &Ruleset, map_text: &str, mode: Mode, seat: Seat, names: [&str; 2], team: i32) -> Value {
    let named = |names: &[String], i: usize| names.get(i).cloned().unwrap_or_default();
    let c = &rules.core;
    json!({
        "map_text": map_text,
        "rules": serde_json::from_str::<Value>(&ibc_engine::rules_json(rules)).unwrap_or(Value::Null),
        "mode": mode.name(),
        "tps": match mode { Mode::Realtime { tps } => tps, Mode::Turn => 0 },
        "unit": seat.unit,
        "team": seat.team,
        "allies": seat.allies,
        "bots": if team == 0 { [names[0], names[1]] } else { [names[1], names[0]] },
        "core": { "walk_speed": c.walk_speed, "run_speed": c.run_speed, "turn_rate": c.turn_rate,
                  "buy_ticks": c.buy_ticks, "round_ticks": c.round_ticks, "team_size": c.team_size },
        "abilities": rules.abilities.iter().enumerate().map(|(i, a)| json!({
            "id": a.id, "name": named(&rules.ability_names, i), "price": a.price, "max_charges": a.max_charges,
            "range": a.range })).collect::<Vec<_>>(),
        "kits": rules.kits.iter().enumerate().map(|(i, k)| json!({
            "id": k.id, "name": named(&rules.kit_names, i), "abilities": k.abilities, "hp": k.hp,
            "speed_pct": k.speed_pct })).collect::<Vec<_>>(),
        "items": rules.items.iter().enumerate().map(|(i, it)| json!({
            "id": it.id, "name": named(&rules.item_names, i), "kind": it.kind, "price": it.price,
            "value": it.value })).collect::<Vec<_>>(),
    })
}

/// Write the run's files and tell the page where they are.
fn finish(session: &Session, out: &ibc_runner::MatchOutput, dir: &Path) -> Result<()> {
    let (replay, _) = job::write_output(dir, out)?;
    let line = job::result_line(&out.summary);
    println!("{line} | replay {}", replay.display());
    session.finish(json!({ "result": line, "replay": replay.display().to_string(), "outcome": out.summary.outcome }));
    Ok(())
}
