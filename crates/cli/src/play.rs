//! `ibc play TEAM_BOT OPPONENT [--map M] [--seed S] [--mode active|realtime|turn] [--tps N] [--team A|B]
//! [--unit SLOT [--kit NAME]] [--port N] [--out DIR] [--viewer-dir DIR]`: play alongside your team's bot
//! against another bot, in the browser, taking over one unit at a time.
//!
//! The match is an ordinary wasmtime match (`ibc_runner::run_driven`). The bot `TEAM_BOT` drafts and plays
//! every unit of your team; the one you possess is driven from the page instead, through the same `act`
//! calls and the same validation, and you can switch to another at any time. At the end the usual
//! `replay.ibcr` and `summary.json` are written to `--out`; your acts are in the replay like any bot's, so
//! `ibc replay verify` re-simulates it. See docs/PLAY.md.

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
#[command(
    name = "ibc play",
    about = "Play alongside your team's bot against another bot, in the browser"
)]
struct Args {
    /// Your team's bot: it drafts and plays every unit of your team you do not possess
    team_bot: PathBuf,
    /// The opposing bot (a bot directory or a .wasm file)
    opponent: PathBuf,
    /// Map file (default: maps/crossfire.ibcmap under the working directory)
    #[arg(long)]
    map: Option<PathBuf>,
    #[arg(long, default_value_t = 0)]
    seed: u64,
    /// active: time passes while you act (Space waits); realtime: ticks run on a clock; turn: one tick per Space
    #[arg(long, value_enum, default_value_t = ModeArg::Active)]
    mode: ModeArg,
    /// Ticks per second in the active and real-time modes
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=60))]
    tps: u32,
    /// Your team: A or B
    #[arg(long, default_value = "A", value_parser = parse_team)]
    team: i32,
    /// The unit of your team you start in (0-based slot; default: choose on the page)
    #[arg(long)]
    unit: Option<u32>,
    /// The kit the unit of --unit drafts, by name, instead of the one the team's bot picks
    #[arg(long, requires = "unit")]
    kit: Option<String>,
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
    Active,
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
    /// The slot the player starts in, if any.
    pub slot: Option<u32>,
    /// The kit, by name, the unit in `slot` drafts in place of the team bot's pick.
    pub kit: Option<String>,
    /// The player's team's bot.
    pub own: BotSpec,
    pub opponent: BotSpec,
}

pub fn run(args: Vec<String>) -> Result<()> {
    let args = Args::parse_from(std::iter::once("ibc play".to_string()).chain(args));
    let root = crate::view::find_viewer_dir(args.viewer_dir.as_deref())?;
    if !root.join("play.html").is_file() {
        bail!(
            "{} has no play.html: rebuild the viewer (`cd viewer && npm run build`)",
            root.display()
        );
    }
    let map = args
        .map
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_MAP));
    let map_text = std::fs::read_to_string(&map)
        .with_context(|| format!("reading map {} (choose one with --map)", map.display()))?;
    let setup = Setup {
        map_text,
        seed: args.seed,
        mode: match args.mode {
            ModeArg::Active => Mode::Active { tps: args.tps },
            ModeArg::Realtime => Mode::Realtime { tps: args.tps },
            ModeArg::Turn => Mode::Turn,
        },
        team: args.team,
        slot: args.unit,
        kit: args.kit.clone(),
        own: bot_spec(&job::resolve_bot(&args.team_bot)?)?,
        opponent: bot_spec(&job::resolve_bot(&args.opponent)?)?,
    };
    let (cfg, session) = prepare(setup)?;

    let listener = TcpListener::bind(("127.0.0.1", args.port))
        .with_context(|| format!("listening on 127.0.0.1:{}", args.port))?;
    println!("http://127.0.0.1:{}/", listener.local_addr()?.port());
    eprintln!(
        "you play team {}; serving {} (Ctrl-C to stop)",
        team_name(session.seat.team),
        root.display()
    );
    let site = Arc::new(server::Site {
        root,
        session: Arc::clone(&session),
    });
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

/// The match config and the session for a setup. Checks the starting unit against the map's team size.
pub fn prepare(s: Setup) -> Result<(MatchConfig, Arc<Session>)> {
    let rules = Ruleset::builtin();
    let map = Map::parse(&s.map_text).map_err(|e| anyhow!("map: {e}"))?;
    let start = Match::new(rules.clone(), map, s.seed);
    let team_size = start.world.team_size as u32;
    if let Some(slot) = s.slot.filter(|&n| n >= team_size) {
        bail!(
            "--unit {slot} does not exist: teams have {team_size} units (0..{})",
            team_size - 1
        );
    }
    let seat = Seat {
        team: s.team,
        team_size,
    };
    let draft = match (&s.kit, s.slot) {
        (Some(name), Some(slot)) => {
            let kit = rules
                .kit_names
                .iter()
                .position(|n| n == name)
                .ok_or_else(|| {
                    anyhow!(
                        "--kit {name} does not exist: the kits are {}",
                        rules.kit_names.join(", ")
                    )
                })?;
            Some((seat.unit(slot), kit as i32))
        }
        (Some(_), None) => bail!("--kit needs --unit: it is the kit of the unit you start in"),
        (None, _) => None,
    };
    let human = BotSpec {
        name: format!("human+{}", s.own.name),
        ..s.own
    };
    let setup_json = setup_json(
        &rules,
        &s.map_text,
        s.mode,
        seat,
        [&human.name, &s.opponent.name],
    );
    let bots = if s.team == 0 {
        [human, s.opponent]
    } else {
        [s.opponent, human]
    };
    let cfg = MatchConfig {
        rules,
        map_text: s.map_text,
        seed: s.seed,
        bots,
        log_limit: ibc_runner::DEFAULT_LOG_LIMIT,
        max_ticks: None,
        frames: false,
    };
    Ok((
        cfg,
        Session::new(s.mode, seat, setup_json, &start, s.slot, draft),
    ))
}

/// What the page needs once: the map, the rules it draws with, names for ids, the team and the mode.
fn setup_json(rules: &Ruleset, map_text: &str, mode: Mode, seat: Seat, names: [&str; 2]) -> Value {
    let named = |names: &[String], i: usize| names.get(i).cloned().unwrap_or_default();
    let c = &rules.core;
    json!({
        "map_text": map_text,
        "rules": serde_json::from_str::<Value>(&ibc_engine::rules_json(rules)).unwrap_or(Value::Null),
        "mode": mode.name(),
        "tps": mode.tps(),
        "team": seat.team,
        "team_size": seat.team_size,
        "first_unit": seat.unit(0),
        "bots": if seat.team == 0 { [names[0], names[1]] } else { [names[1], names[0]] },
        "core": { "walk_speed": c.walk_speed, "run_speed": c.run_speed, "turn_rate": c.turn_rate,
                  "buy_ticks": c.buy_ticks, "round_ticks": c.round_ticks, "team_size": c.team_size,
                  "pickup_reach": c.pickup_reach },
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
