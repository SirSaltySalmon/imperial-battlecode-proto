//! Wasmtime host: runs a match between two wasm bots on `ibc-engine` and records the replay.
//!
//! Bots are `wasm32-wasip1` command modules. Every unit gets its own instance of its team's module;
//! `_start` never returns and calls the import `ibc.end_turn` once per tick. See `vm` for the fuel model,
//! `stack` for the recursion limit and `wasi` for the (deterministic, hand-written) WASI shim.

mod stack;
mod vm;
mod wasi;

use anyhow::{anyhow, bail, Context as _, Result};
use ibc_engine::{Map, Match, Ruleset};
use ibc_replay::{
    hash_hex, sha256_hex, BotInfo, Fault, Header, HeaderRules, MatchResult, Record, Replay, ReplayWriter, Summary,
    TeamFuel, TeamSummary,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use vm::{lock, TurnEnd, UnitData, UnitVm};
use wasmtime::{Collector, Config, Engine, ExternType, Linker, Module};

pub use vm::IBC_IMPORTS;
pub use wasi::SUPPORTED as WASI_IMPORTS;

/// Exact version of the `wasmtime` crate this host is built with (pinned with `=` in Cargo.toml).
pub const WASMTIME_VERSION: &str = "51.0.0";
/// Default cap of a unit's captured log.
pub const DEFAULT_LOG_LIMIT: usize = 256 << 10;
/// Largest `stack_limit` a ruleset may set: the native stack below is sized for it. Twice the default rule,
/// so a balance patch can raise it without a runner change.
pub const MAX_STACK_UNITS: u32 = 1 << 17;
/// Native stack per stack unit. Wasmtime traps on its own when wasm frames use more than
/// `MAX_STACK_UNITS * NATIVE_BYTES_PER_UNIT` bytes, and that must never come before the deterministic
/// limit. Measured on x64 (`tests/stack.rs`): 8 bytes per unit for thin and i64 frames, at most 15.5 for
/// v128 frames, whose spill slots are as wide on aarch64. 64 is four times the worst measured, eight times at
/// the default `stack_limit`. A frame far larger than its stack units (Cranelift keeping many recomputable
/// values alive across a call) could still exhaust it first; such a bot faults with "call stack exhausted".
const NATIVE_BYTES_PER_UNIT: usize = 64;
/// Stack left on a unit's fiber for host calls below its deepest wasm frame (Wasmtime's default margin).
const HOST_STACK_BYTES: usize = 3 << 19;

pub struct BotSpec {
    pub name: String,
    pub wasm: Vec<u8>,
    /// The bot's language, which picks its row of the ruleset's `[[language]]` table (fuel and memory
    /// multipliers); a language without a row gets the `default` row. `language_of` reads it from a module.
    pub language: String,
    /// Replaces the fuel multiplier of the language's row (a local experiment, a bench job). Resolution 0.001.
    pub fuel_override: Option<f64>,
}

impl BotSpec {
    /// A bot whose language is what its module says (`language_of`), with no override.
    pub fn new(name: impl Into<String>, wasm: Vec<u8>) -> BotSpec {
        let language = language_of(&wasm);
        BotSpec { name: name.into(), wasm, language, fuel_override: None }
    }
}

/// The language a module names, as the coordinator's upload check reads it (`ibc_replay::detect_languages`):
/// the one it names if it names exactly one, else `default`.
pub fn language_of(wasm: &[u8]) -> String {
    let named = ibc_replay::detect_languages(wasm);
    match (named.len(), named.into_iter().next()) {
        (1, Some(only)) => only,
        _ => ibc_engine::language::DEFAULT_LANGUAGE.to_string(),
    }
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
    /// `replay.encode()`: the bytes of `replay.ibcr`, which `summary.replay_sha256` hashes. Write these rather
    /// than encoding again; the deflate is a large share of a match's cost.
    pub replay_bytes: Vec<u8>,
    pub summary: Summary,
    /// Captured stdout/stderr/`ibc.log` output, indexed by unit id. The replay has the same bytes per turn, but
    /// not the `[ibc] log truncated` notice.
    pub logs: Vec<Vec<u8>>,
    pub frames: Option<Vec<String>>,
    /// Indexed by unit id.
    pub units: Vec<UnitStats>,
}

struct HostEnv {
    engine: Engine,
    linker: Linker<UnitData>,
    /// Compiled modules by wasm sha256 and stack limit, shared by every match in this process.
    modules: Mutex<HashMap<(String, u32), Module>>,
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
            // Recursion is limited deterministically (`stack`); the native stack only has to be larger.
            let wasm_stack = MAX_STACK_UNITS as usize * NATIVE_BYTES_PER_UNIT;
            c.max_wasm_stack(wasm_stack);
            c.async_stack_size(wasm_stack + HOST_STACK_BYTES);
            // (wasmtime is built without its `threads` feature, so the threads proposal cannot be enabled.)
            // Exception handling, for C++ bots built with exceptions: the standard (exnref) encoding only,
            // Wasmtime cannot compile the legacy one. The coordinator's upload check accepts the same
            // (`ibc_coordinator::wasmcheck::FEATURES`). `throw` and `try_table` cost 1 fuel like any
            // instruction; unwinding and the catch are free. Exception objects live in a per-store GC heap
            // with the null collector: a throw is a bump allocation, nothing is ever collected, so no
            // collection can suspend the bot mid-turn (the driver would take that for running out of fuel).
            // The heap has the same cap as the unit's linear memory, separately (`memory_bytes`, 8 MiB): at
            // 24 bytes per throw that is about 350,000 throws per unit per match, then a throw traps
            // ("allocation size too large"). The GC proposal itself (structs, arrays) stays off.
            c.wasm_exceptions(true);
            c.wasm_gc(false);
            c.collector(Collector::Null);
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

/// Validate a bot module, limit its stack to `stack_limit` units (`stack`), compile it and check its imports
/// and exports.
fn compile(env: &HostEnv, wasm: &[u8], stack_limit: u32) -> Result<Module> {
    wt(Module::validate(&env.engine, wasm)).context("invalid wasm")?;
    let limited = stack::instrument(wasm, stack_limit).map_err(|e| anyhow!("cannot limit its stack: {e}"))?;
    let module = wt(Module::new(&env.engine, &limited)).context("invalid wasm after limiting its stack")?;
    check_module(&module).map_err(|e| anyhow!(e))?;
    Ok(module)
}

/// Compile (or fetch from the process-wide cache) and validate a bot module. Returns it with its sha256.
fn load_module(env: &HostEnv, bot: &BotSpec, stack_limit: u32) -> Result<(Module, String)> {
    let sha = sha256_hex(&bot.wasm);
    let key = (sha.clone(), stack_limit);
    if let Some(m) = env.modules.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Ok((m.clone(), sha));
    }
    let module = compile(env, &bot.wasm, stack_limit).map_err(|e| anyhow!("bot `{}`: {e:#}", bot.name))?;
    let mut cache = env.modules.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= MODULE_CACHE_ENTRIES {
        cache.clear();
    }
    cache.insert(key, module.clone());
    Ok((module, sha))
}

/// Validate a bot without running it: compiles it as a match would and checks imports/exports.
pub fn check_wasm(wasm: &[u8]) -> Result<()> {
    compile(host()?, wasm, MAX_STACK_UNITS).map(drop)
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

/// Drives some units from outside the runner (a human at a keyboard, a test script) instead of their team's
/// wasm bot. A driven unit acts through the same `ibc_abi::Host` calls as a bot, at its place in the turn
/// order, so the engine validates it the same way and the replay records it the same way.
pub trait Driver {
    /// True if this unit is always driven here and never runs its team's bot. Asked once per unit before
    /// tick 0. A team whose units are all driven needs no module: its `BotSpec::wasm` is not loaded and the
    /// replay header gives it an empty sha256.
    fn drives(&self, unit: u32, team: i32) -> bool;
    /// Sees the match before the first tick and after every step. May block (a human thinking, a tick
    /// clock). Returning false stops the match there; it is then recorded as unfinished.
    fn observe(&mut self, game: &Match) -> bool;
    /// Offered every unit's turn, at its place in the turn order. Returns true if it took the turn (issued
    /// the unit's acts on `game` through `ibc_abi::Host`, or chose to make none): the unit's bot then does
    /// not run this tick and resumes where it was on a later one. The answer is ignored for units `drives`
    /// claimed, which never run a bot.
    fn turn(&mut self, unit: u32, game: &mut Match) -> bool;
}

/// No unit is driven from outside: every unit runs its team's bot.
struct Bots;

impl Driver for Bots {
    fn drives(&self, _unit: u32, _team: i32) -> bool {
        false
    }
    fn observe(&mut self, _game: &Match) -> bool {
        true
    }
    fn turn(&mut self, _unit: u32, _game: &mut Match) -> bool {
        false
    }
}

/// The module of each team and its sha256, loaded only if one of the team's units runs it (not driven).
fn load_team_modules(
    env: &HostEnv,
    bots: &[BotSpec; 2],
    teams: &[i32],
    driven: &[bool],
    stack_limit: u32,
) -> Result<([Option<Module>; 2], [String; 2])> {
    let mut modules = [None, None];
    let mut shas = [String::new(), String::new()];
    for t in 0..2 {
        if (0..teams.len()).any(|u| !driven[u] && teams[u] == t as i32) {
            let (module, sha) = load_module(env, &bots[t], stack_limit)?;
            modules[t] = Some(module);
            shas[t] = sha;
        }
    }
    Ok((modules, shas))
}

pub fn run_match(cfg: MatchConfig) -> Result<MatchOutput> {
    run_driven(cfg, &mut Bots)
}

/// `run_match` with some units driven by `driver` instead of their bots (see `Driver`).
pub fn run_driven(cfg: MatchConfig, driver: &mut dyn Driver) -> Result<MatchOutput> {
    let env = host()?;
    let map = Map::parse(&cfg.map_text).map_err(|e| anyhow!("map: {e}"))?;
    // Each team's allowances: its language's row, the fuel multiplier unless the run overrides it.
    let rows = [cfg.rules.language(&cfg.bots[0].language), cfg.rules.language(&cfg.bots[1].language)];
    let fuel_milli = |t: usize| match cfg.bots[t].fuel_override {
        Some(m) => milli(m),
        None => Ok(rows[t].fuel_milli as u64),
    };
    let mults = [fuel_milli(0)?, fuel_milli(1)?];
    let stack_limit = cfg.rules.stack_limit as u32;
    if stack_limit > MAX_STACK_UNITS {
        bail!("rules: stack_limit {stack_limit} is above what this runner supports ({MAX_STACK_UNITS})");
    }
    let ruleset_hash = hash_hex(cfg.rules.hash());
    let header_rules = HeaderRules::of(&cfg.rules);
    let map_hash = hash_hex(map.hash());
    // Each team's linear memory cap per unit (`memory_bytes` times its language's row), read before the
    // ruleset moves into the match.
    let memory_limit = rows.clone().map(|r| usize::try_from(r.memory_cap(cfg.rules.memory_bytes)).unwrap_or(usize::MAX));
    let game = Arc::new(Mutex::new(Match::new(cfg.rules, map, cfg.seed)));
    let n = lock(&game).unit_count();
    let teams: Vec<i32> = (0..n).map(|u| lock(&game).unit_team(u)).collect();
    let driven: Vec<bool> = (0..n).map(|u| driver.drives(u, teams[u as usize])).collect();
    let (modules, shas) = load_team_modules(env, &cfg.bots, &teams, &driven, stack_limit)?;

    let bot_info = |i: usize| BotInfo {
        name: cfg.bots[i].name.clone(),
        sha256: shas[i].clone(),
        // what was actually applied
        fuel_multiplier: mults[i] as f64 / 1000.0,
        language: cfg.bots[i].language.clone(),
        memory_multiplier: rows[i].memory_milli as f64 / 1000.0,
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
        team_size: n / 2,
        redacted: [false, false],
        rules: header_rules,
    };
    let mut writer = ReplayWriter::new(header);

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
        let team = units[unit as usize].team as usize;
        writer.record(team, Record::Log { unit, text: format!("{note}\n").into_bytes() });
        logs[unit as usize] = match vm {
            Some(vm) => vm.finish(Some(&note)),
            None => format!("{note}\n").into_bytes(),
        };
        units[unit as usize].faulted = true;
        faults.push(Fault { unit, team: units[unit as usize].team, tick, reason });
    };

    for unit in 0..n {
        let team = teams[unit as usize];
        if !(0..=1).contains(&team) {
            bail!("engine reported team {team} for unit {unit}");
        }
        units.push(UnitStats { team, ..Default::default() });
        let module = match &modules[team as usize] {
            Some(m) if !driven[unit as usize] => m,
            _ => {
                vms.push(None);
                continue;
            }
        };
        match UnitVm::new(
            &env.engine,
            &env.linker,
            module,
            game.clone(),
            unit,
            cfg.seed,
            cfg.log_limit,
            memory_limit[team as usize],
            stack_limit,
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
    let mut go_on = driver.observe(&lock(&game));
    // Bytes of each unit's log already recorded in the replay.
    let mut log_seen: Vec<usize> = vec![0; n as usize];

    loop {
        let order = {
            let g = lock(&game);
            if !go_on || g.finished() || cfg.max_ticks.is_some_and(|m| writer.ticks() >= m) {
                break;
            }
            g.turn_order()
        };
        let mut tick_fuel = [0u64; 2];
        for unit in order {
            if driver.turn(unit, &mut lock(&game)) || driven[unit as usize] {
                continue;
            }
            let Some(vm) = vms.get_mut(unit as usize).and_then(|v| v.as_mut()) else { continue };
            let team = units[unit as usize].team as usize;
            let budget = scale(lock(&game).fuel_budget(unit), mults[team]);
            let turn = vm.run_turn(budget);
            let text = vm.log_since(log_seen[unit as usize]);
            log_seen[unit as usize] += text.len();
            if !text.is_empty() {
                writer.record(team, Record::Log { unit, text });
            }
            let out_of_fuel = matches!(turn.end, TurnEnd::OutOfFuel);
            writer.record(team, Record::Fuel { unit, used: turn.used, out_of_fuel });

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
        writer.draws(g.take_debug());
        g.step();
        let hashes = g.state_hashes();
        final_hash = hashes.combined();
        writer.end_tick(hashes);
        if let Some(f) = frames.as_mut() {
            f.push(g.frame_json());
        }
        go_on = driver.observe(&g);
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
    let replay_bytes = replay.encode();
    let replay_sha256 = sha256_hex(&replay_bytes);
    for (t, f) in team_fuel.iter_mut().enumerate() {
        f.set_turn_percentiles(&replay, t);
    }
    let team = |i: usize| TeamSummary {
        name: replay.header.bots[i].name.clone(),
        wasm_sha256: replay.header.bots[i].sha256.clone(),
        fuel_multiplier: replay.header.bots[i].fuel_multiplier,
        language: replay.header.bots[i].language.clone(),
        memory_multiplier: replay.header.bots[i].memory_multiplier,
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
    Ok(MatchOutput { replay, replay_bytes, summary, logs, frames, units })
}
