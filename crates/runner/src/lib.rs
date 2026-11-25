//! Wasmtime host: runs a match between two wasm bots on `ibc-engine` and records the replay.
//!
//! Bots are `wasm32-wasip1` command modules. Every unit gets its own instance of its team's module;
//! `_start` never returns and calls the import `ibc.end_turn` once per tick. See `vm` for the fuel model
//! and `wasi` for the (deterministic, hand-written) WASI shim.

mod vm;
mod wasi;

use anyhow::{anyhow, bail, Context as _, Result};
use ibc_engine::{Map, Match, Ruleset};
use ibc_replay::{
    hash_hex, sha256_hex, BotInfo, Fault, Header, MatchResult, Replay, ReplayWriter, Summary, TeamFuel, TeamSummary,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use vm::{lock, TurnEnd, UnitData, UnitVm};
use wasmtime::{Config, Engine, ExternType, Linker, Module};

pub use vm::IBC_IMPORTS;
pub use wasi::SUPPORTED as WASI_IMPORTS;

/// Exact version of the `wasmtime` crate this host is built with (pinned with `=` in Cargo.toml).
pub const WASMTIME_VERSION: &str = "51.0.0";
/// Linear memory cap per bot instance.
pub const MEMORY_LIMIT_BYTES: usize = 64 << 20;
/// Default cap of a unit's captured log.
pub const DEFAULT_LOG_LIMIT: usize = 256 << 10;

pub struct BotSpec {
    pub name: String,
    pub wasm: Vec<u8>,
    /// Language multiplier on the engine's per-tick fuel budget (1.0 for Rust). Resolution 0.001.
    pub fuel_multiplier: f64,
}

pub struct MatchConfig {
    pub rules: Ruleset,
    pub map_text: String,
    pub seed: u64,
    /// Team A, team B.
    pub bots: [BotSpec; 2],
    /// Byte cap of each unit's log.
    pub log_limit: usize,
    /// Stop after this many ticks even if the engine has not ended the match.
    pub max_ticks: Option<u32>,
    /// Collect `Match::frame_json()` before the first tick and after every step.
    pub frames: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnitStats {
    pub team: i32,
    /// Fuel consumed (wasm instructions + host call charges).
    pub fuel_used: u64,
    /// Sum of the budgets of the turns this unit was scheduled for.
    pub fuel_granted: u64,
    pub turns: u64,
    pub out_of_fuel_turns: u64,
    /// Most fuel used in one turn.
    pub max_turn: u64,
    pub faulted: bool,
}

pub struct MatchOutput {
    pub replay: Replay,
    pub summary: Summary,
    /// Captured stdout/stderr/`ibc.log` output, indexed by unit id.
    pub logs: Vec<Vec<u8>>,
    pub frames: Option<Vec<String>>,
    /// Indexed by unit id.
    pub units: Vec<UnitStats>,
}

struct HostEnv {
    engine: Engine,
    linker: Linker<UnitData>,
    /// Compiled modules by wasm sha256, shared by every match in this process.
    modules: Mutex<HashMap<String, Module>>,
}

const MODULE_CACHE_ENTRIES: usize = 64;

fn wt<T>(r: wasmtime::Result<T>) -> Result<T> {
    r.map_err(|e| anyhow!("{e:#}"))
}

fn host() -> Result<&'static HostEnv> {
    static HOST: OnceLock<std::result::Result<HostEnv, String>> = OnceLock::new();
    HOST.get_or_init(|| {
        let build = || -> wasmtime::Result<HostEnv> {
            let mut c = Config::new();
            // Determinism: metered execution, canonical NaNs, deterministic relaxed-simd, no threads.
            c.consume_fuel(true);
            c.cranelift_nan_canonicalization(true);
            c.relaxed_simd_deterministic(true);
            // (wasmtime is built without its `threads` feature, so the threads proposal cannot be enabled.)
            let engine = Engine::new(&c)?;
            let mut linker = Linker::new(&engine);
            vm::add_to_linker(&mut linker)?;
            wasi::add_to_linker(&mut linker)?;
            Ok(HostEnv { engine, linker, modules: Mutex::new(HashMap::new()) })
        };
        build().map_err(|e| format!("{e:#}"))
    })
    .as_ref()
    .map_err(|e| anyhow!("wasmtime setup failed: {e}"))
}

/// Check that a module only needs what this host provides. The error lists every offending import.
fn check_module(module: &Module) -> std::result::Result<(), String> {
    let mut bad = Vec::new();
    for imp in module.imports() {
        let known = match imp.module() {
            m if m == ibc_abi::IMPORT_MODULE => IBC_IMPORTS.contains(&imp.name()),
            wasi::MODULE => WASI_IMPORTS.contains(&imp.name()),
            _ => false,
        };
        if !known || !matches!(imp.ty(), ExternType::Func(_)) {
            bad.push(format!("{}.{}", imp.module(), imp.name()));
        }
    }
    if !bad.is_empty() {
        return Err(format!("unsupported imports: {}", bad.join(", ")));
    }
    match module.get_export("_start") {
        Some(ExternType::Func(f)) if f.params().len() == 0 && f.results().len() == 0 => {}
        _ => return Err("missing export `_start` of type () -> () (build a wasm32-wasip1 command module)".into()),
    }
    if !matches!(module.get_export("memory"), Some(ExternType::Memory(_))) {
        return Err("missing export `memory`".into());
    }
    Ok(())
}

/// Compile (or fetch from the process-wide cache) and validate a bot module. Returns it with its sha256.
fn load_module(env: &HostEnv, bot: &BotSpec) -> Result<(Module, String)> {
    let sha = sha256_hex(&bot.wasm);
    if let Some(m) = env.modules.lock().unwrap_or_else(|e| e.into_inner()).get(&sha) {
        return Ok((m.clone(), sha));
    }
    let module = wt(Module::new(&env.engine, &bot.wasm)).with_context(|| format!("bot `{}`: invalid wasm", bot.name))?;
    check_module(&module).map_err(|e| anyhow!("bot `{}`: {e}", bot.name))?;
    let mut cache = env.modules.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= MODULE_CACHE_ENTRIES {
        cache.clear();
    }
    cache.insert(sha.clone(), module.clone());
    Ok((module, sha))
}

/// Validate a bot without running it: compiles and checks imports/exports.
pub fn check_wasm(wasm: &[u8]) -> Result<()> {
    let env = host()?;
    let module = wt(Module::new(&env.engine, wasm)).context("invalid wasm")?;
    check_module(&module).map_err(|e| anyhow!(e))
}

fn milli(mult: f64) -> Result<u64> {
    if !(mult.is_finite() && (0.001..=1000.0).contains(&mult)) {
        bail!("fuel multiplier {mult} is out of range (0.001 ..= 1000)");
    }
    Ok((mult * 1000.0).round() as u64)
}

/// The fuel a unit gets this tick: engine budget times the language multiplier (in 1/1000), at least 1.
pub fn scaled_budget(base: u64, fuel_multiplier: f64) -> Result<u64> {
    Ok(scale(base, milli(fuel_multiplier)?))
}

fn scale(base: u64, milli: u64) -> u64 {
    ((base as u128 * milli as u128 / 1000).min((u64::MAX / 4) as u128) as u64).max(1)
}

pub fn run_match(cfg: MatchConfig) -> Result<MatchOutput> {
    let env = host()?;
    let map = Map::parse(&cfg.map_text).map_err(|e| anyhow!("map: {e}"))?;
    let mults = [milli(cfg.bots[0].fuel_multiplier)?, milli(cfg.bots[1].fuel_multiplier)?];
    let (module_a, sha_a) = load_module(env, &cfg.bots[0])?;
    let (module_b, sha_b) = load_module(env, &cfg.bots[1])?;
    let modules = [module_a, module_b];
    let shas = [sha_a, sha_b];

    let ruleset_hash = hash_hex(cfg.rules.hash());
    let map_hash = hash_hex(map.hash());
    let bot_info = |i: usize| BotInfo {
        name: cfg.bots[i].name.clone(),
        sha256: shas[i].clone(),
        // what was actually applied
        fuel_multiplier: mults[i] as f64 / 1000.0,
    };
    let header = Header {
        engine_version: ibc_engine::ENGINE_VERSION.to_string(),
        ruleset_hash: ruleset_hash.clone(),
        map_hash: map_hash.clone(),
        map_text: cfg.map_text.clone(),
        seed: cfg.seed,
        wasmtime_version: WASMTIME_VERSION.to_string(),
        abi_version: ibc_abi::ABI_VERSION,
        bots: [bot_info(0), bot_info(1)],
        result: MatchResult { winner: None, score: [0, 0], ticks: 0, finished: false },
    };
    let mut writer = ReplayWriter::new(header);

    let game = Arc::new(Mutex::new(Match::new(cfg.rules, map, cfg.seed)));
    let n = lock(&game).unit_count();
    let mut units: Vec<UnitStats> = Vec::with_capacity(n as usize);
    let mut vms: Vec<Option<UnitVm>> = Vec::with_capacity(n as usize);
    let mut logs: Vec<Vec<u8>> = vec![Vec::new(); n as usize];
    let mut faults: Vec<Fault> = Vec::new();

    // Records a fault at the current point of the current tick, in the replay and in the engine.
    let fault = |unit: u32,
                     reason: String,
                     vm: Option<UnitVm>,
                     writer: &mut ReplayWriter,
                     units: &mut Vec<UnitStats>,
                     logs: &mut Vec<Vec<u8>>,
                     faults: &mut Vec<Fault>| {
        let mut g = lock(&game);
        // Inputs accepted so far stay ahead of the fault in the replay, exactly as they happened.
        writer.inputs(g.take_inputs());
        writer.fault(unit);
        g.fault(unit);
        let tick = g.tick();
        drop(g);
        let note = format!("[ibc] unit {unit} faulted at tick {tick}: {reason}");
        logs[unit as usize] = match vm {
            Some(vm) => vm.finish(Some(&note)),
            None => format!("{note}\n").into_bytes(),
        };
        units[unit as usize].faulted = true;
        faults.push(Fault { unit, team: units[unit as usize].team, tick, reason });
    };

    for unit in 0..n {
        let team = lock(&game).unit_team(unit);
        if !(0..=1).contains(&team) {
            bail!("engine reported team {team} for unit {unit}");
        }
        units.push(UnitStats { team, ..Default::default() });
        match UnitVm::new(
            &env.engine,
            &env.linker,
            &modules[team as usize],
            game.clone(),
            unit,
            cfg.seed,
            cfg.log_limit,
            MEMORY_LIMIT_BYTES,
        ) {
            Ok(vm) => vms.push(Some(vm)),
            Err(reason) => {
                vms.push(None);
                fault(unit, reason, None, &mut writer, &mut units, &mut logs, &mut faults);
            }
        }
    }

    let mut frames = cfg.frames.then(|| vec![lock(&game).frame_json()]);
    let mut team_fuel = [TeamFuel::default(), TeamFuel::default()];
    let mut final_hash = lock(&game).state_hash();

    loop {
        let order = {
            let g = lock(&game);
            if g.finished() || cfg.max_ticks.is_some_and(|m| writer.ticks() >= m) {
                break;
            }
            g.turn_order()
        };
        let mut tick_fuel = [0u64; 2];
        for unit in order {
            let Some(vm) = vms.get_mut(unit as usize).and_then(|v| v.as_mut()) else { continue };
            let team = units[unit as usize].team as usize;
            let budget = scale(lock(&game).fuel_budget(unit), mults[team]);
            let turn = vm.run_turn(budget);

            let s = &mut units[unit as usize];
            s.fuel_used += turn.used;
            s.fuel_granted += budget;
            s.turns += 1;
            s.max_turn = s.max_turn.max(turn.used);
            tick_fuel[team] += turn.used;
            match turn.end {
                TurnEnd::Yielded => {}
                TurnEnd::OutOfFuel => s.out_of_fuel_turns += 1,
                TurnEnd::Fault(reason) => {
                    let vm = vms[unit as usize].take();
                    fault(unit, reason, vm, &mut writer, &mut units, &mut logs, &mut faults);
                }
            }
        }
        for t in 0..2 {
            team_fuel[t].max_per_tick = team_fuel[t].max_per_tick.max(tick_fuel[t]);
        }
        let mut g = lock(&game);
        writer.inputs(g.take_inputs());
        g.step();
        final_hash = g.state_hash();
        writer.end_tick(final_hash);
        if let Some(f) = frames.as_mut() {
            f.push(g.frame_json());
        }
    }

    for (unit, vm) in vms.into_iter().enumerate() {
        if let Some(vm) = vm {
            logs[unit] = vm.finish(None);
        }
    }

    let g = lock(&game);
    let result = MatchResult { winner: g.winner(), score: g.score(), ticks: writer.ticks(), finished: g.finished() };
    let game_summary: serde_json::Value = serde_json::from_str(&g.summary_json()).unwrap_or(serde_json::Value::Null);
    drop(g);

    for s in &units {
        let f = &mut team_fuel[s.team as usize];
        f.total += s.fuel_used;
        f.granted += s.fuel_granted;
        f.turns += s.turns;
        f.out_of_fuel_turns += s.out_of_fuel_turns;
        f.max_unit_turn = f.max_unit_turn.max(s.max_turn);
    }
    let replay = writer.finish(result.clone());
    let replay_sha256 = sha256_hex(&replay.encode());
    let team = |i: usize| TeamSummary {
        name: replay.header.bots[i].name.clone(),
        wasm_sha256: replay.header.bots[i].sha256.clone(),
        fuel_multiplier: replay.header.bots[i].fuel_multiplier,
        fuel: team_fuel[i].clone(),
        faults: faults.iter().filter(|f| f.team == i as i32).count() as u32,
    };
    let summary = Summary {
        outcome: Summary::outcome_of(&result).to_string(),
        winner: result.winner,
        score: result.score,
        ticks: result.ticks,
        teams: [team(0), team(1)],
        faults,
        seed: cfg.seed,
        engine_version: ibc_engine::ENGINE_VERSION.to_string(),
        wasmtime_version: WASMTIME_VERSION.to_string(),
        abi_version: ibc_abi::ABI_VERSION,
        ruleset_hash,
        map_hash,
        final_state_hash: hash_hex(final_hash),
        replay_sha256,
        game: game_summary,
    };
    Ok(MatchOutput { replay, summary, logs, frames, units })
}
