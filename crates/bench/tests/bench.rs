use ibc_bench::backend::{record_native, run_games, BackendKind, RunOpts};
use ibc_bench::model::{load_maps, paired_schedule, Bot, Game, GameResult, MapEntry, RoundRec};
use ibc_bench::stats::{Sprt, SprtDecision};
use ibc_bench::{ab, balance, compare, dump, league, noise, report};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn maps() -> Vec<MapEntry> {
    load_maps(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../maps"
    )))
    .expect("maps directory")
}

fn bots(names: [&str; 2]) -> [Bot; 2] {
    let pool = Bot::parse_pool(&names.map(String::from), Path::new("no-such-dir")).unwrap();
    [pool[0].clone(), pool[1].clone()]
}

fn opts(jobs: usize, detail: bool) -> RunOpts {
    let mut o = RunOpts::new(PathBuf::from("unused"));
    o.backend = BackendKind::Native;
    o.jobs = jobs;
    o.detail = detail;
    o
}

/// A hand-made result in `paired_schedule` order: even ids have A in the team A slot.
fn fake(id: usize, map: &str, seed: u64, a_rounds: i32, b_rounds: i32) -> GameResult {
    let game = Game {
        id,
        map: 0,
        seed,
        a: id % 2,
        b: 1 - id % 2,
    };
    let names = if id % 2 == 0 { ("x", "y") } else { ("y", "x") };
    let mut r = GameResult::failed(&game, map, names.0, names.1, "native", String::new());
    r.error = None;
    let slot_of_a = id % 2;
    r.score[slot_of_a] = a_rounds;
    r.score[1 - slot_of_a] = b_rounds;
    r.winner = match a_rounds.cmp(&b_rounds) {
        std::cmp::Ordering::Greater => Some(slot_of_a as i32),
        std::cmp::Ordering::Less => Some(1 - slot_of_a as i32),
        std::cmp::Ordering::Equal => None,
    };
    // Make every game unique so nothing is dropped as an exact repeat.
    r.ticks = 1000 + id as u32;
    r.rounds = vec![RoundRec {
        round: 1,
        winner: 0,
        reason: 1,
        attackers: 0,
        ticks: id as i32,
    }];
    r
}

#[test]
fn paired_analysis_counts_and_calls_noise_noise() {
    // Six pairs: A sweeps one, loses one, and splits four (one win each).
    let mut results = Vec::new();
    let outcomes = [
        ((7, 3), (7, 5)),
        ((2, 7), (4, 7)),
        ((7, 6), (5, 7)),
        ((7, 1), (6, 7)),
        ((3, 7), (7, 2)),
        ((7, 0), (0, 7)),
    ];
    for (k, (g1, g2)) in outcomes.into_iter().enumerate() {
        results.push(fake(2 * k, "m", k as u64, g1.0, g1.1));
        results.push(fake(2 * k + 1, "m", k as u64, g2.0, g2.1));
    }
    let r = ab::analyse("x", "y", &results, None, Vec::new());
    assert_eq!((r.games, r.wins, r.draws, r.losses), (12, 6, 0, 6));
    assert_eq!((r.pairs.played, r.pairs.distinct), (6, 6));
    assert_eq!(
        (r.pairs.a_better, r.pairs.equal, r.pairs.b_better),
        (1, 4, 1)
    );
    assert_eq!(r.pairs.sign_p, 1.0);
    assert_eq!(r.score.value, 0.5);
    assert_eq!((r.rounds.a, r.rounds.b), (62, 59));
    assert_eq!(r.verdict, "inside_noise");
    assert!(
        r.verdict_text.starts_with("INSIDE THE NOISE"),
        "{}",
        r.verdict_text
    );
    assert_eq!(r.lost_games.len(), 6);

    // Eight clean sweeps are a real difference; the mirror image favours B.
    let sweep = |a: (i32, i32)| -> Vec<GameResult> {
        (0..16)
            .map(|id| {
                fake(
                    id,
                    if id % 4 < 2 { "m1" } else { "m2" },
                    (id / 2) as u64,
                    a.0,
                    a.1,
                )
            })
            .collect()
    };
    let r = ab::analyse("x", "y", &sweep((7, 2)), None, Vec::new());
    assert_eq!(
        (r.verdict.as_str(), r.pairs.a_better, r.wins),
        ("a_better", 8, 16)
    );
    assert!(r.pairs.sign_p < 0.01 && r.verdict_text.starts_with("REAL DIFFERENCE: x"));
    assert_eq!(r.per_map.len(), 2);
    assert_eq!(
        ab::analyse("x", "y", &sweep((2, 7)), None, Vec::new()).verdict,
        "b_better"
    );
    // Four sweeps are suggestive but too few to call.
    let few = ab::analyse("x", "y", &sweep((7, 2))[..8], None, Vec::new());
    assert_eq!(few.verdict, "inside_noise");
    assert!(
        few.verdict_text.starts_with("NOT ESTABLISHED: x is ahead"),
        "{}",
        few.verdict_text
    );
}

#[test]
fn exact_repeats_are_not_evidence() {
    // The same pair played under ten seeds is one observation, and one observation measures no noise.
    let mut results = Vec::new();
    for k in 0..10 {
        for id in [2 * k, 2 * k + 1] {
            let mut r = fake(id, "m", k as u64, 7, 0);
            r.ticks = 500;
            r.rounds.clear();
            results.push(r);
        }
    }
    let r = ab::analyse("x", "y", &results, None, Vec::new());
    assert_eq!((r.pairs.played, r.pairs.distinct, r.wins), (10, 1, 20));
    assert_eq!(r.verdict, "inside_noise");
    assert!(
        r.warnings.iter().any(|w| w.contains("exact repeats")),
        "{:?}",
        r.warnings
    );
}

#[test]
fn failed_games_are_excluded_and_reported() {
    let mut results: Vec<GameResult> = (0..8)
        .map(|id| fake(id, "m", (id / 2) as u64, 7, 3))
        .collect();
    results[3].error = Some("boom".into());
    let r = ab::analyse("x", "y", &results, None, Vec::new());
    assert_eq!((r.games, r.failed, r.pairs.played), (7, 1, 3));
    assert!(r.warnings.iter().any(|w| w.contains("failed")));
}

#[test]
fn small_native_ab_is_deterministic() {
    let maps = maps();
    let run = |jobs: usize| {
        let (rep, results) = ab::run_ab(
            &bots(["rusher", "holder"]),
            &maps,
            &[1, 2],
            None,
            8,
            &opts(jobs, false),
        )
        .unwrap();
        (report::to_json(&rep).unwrap(), results)
    };
    let (a, results_a) = run(1);
    let (b, results_b) = run(3);
    assert_eq!(
        a, b,
        "the report must not depend on thread count or on anything but the inputs"
    );
    assert_eq!(results_a, results_b);
    assert!(
        results_a.iter().all(|r| r.ok()),
        "{:?}",
        results_a
            .iter()
            .filter_map(|r| r.error.clone())
            .collect::<Vec<_>>()
    );
    // Results come back in schedule order whatever order the threads finished in.
    let schedule = paired_schedule(maps.len(), &[1, 2]);
    for (g, r) in schedule.iter().zip(&results_a) {
        assert_eq!(
            (g.id, g.seed, maps[g.map].name.as_str()),
            (r.id, r.seed, r.map.as_str())
        );
    }
    // Same engine, same inputs as the reference harness.
    let sim = ibc_sim::run(
        &maps[0].text,
        1,
        "rusher",
        "holder",
        ibc_sim::DEFAULT_MAX_TICKS,
    )
    .unwrap();
    assert_eq!(results_a[0].final_hash, format!("{:016x}", sim.final_hash));
    assert_eq!(results_a[0].score, sim.score);
}

#[test]
fn sprt_stops_early_and_deterministically() {
    let maps = maps();
    let sprt = Sprt {
        elo0: 0.0,
        elo1: 50.0,
        alpha: 0.05,
        beta: 0.05,
    };
    let seeds: Vec<u64> = (1..=30).collect();
    let run = |jobs| {
        ab::run_ab(
            &bots(["rusher", "idle"]),
            &maps,
            &seeds,
            Some(sprt),
            2,
            &opts(jobs, false),
        )
        .unwrap()
    };
    let (rep, results) = run(2);
    // These bots ignore the seed, so the second batch only repeats the first: the run stops there
    // instead of burning the remaining seeds, and says so rather than claiming a decision.
    assert!(
        results.len() < seeds.len() * maps.len() * 2,
        "played all {} games",
        results.len()
    );
    assert_eq!(results.len() % 4, 0, "stops only on a batch boundary");
    assert_eq!(rep.wins, rep.games);
    // With a small pool the run stops because a batch adds nothing new; with a larger one the SPRT decides.
    assert!(
        rep.sprt.as_ref().is_some_and(|s| !matches!(s.decision, ibc_bench::stats::SprtDecision::Continue))
            || rep.warnings.iter().any(|w| w.contains("no new distinct pair")),
        "{:?}",
        rep.warnings
    );
    assert_eq!(
        report::to_json(&rep).unwrap(),
        report::to_json(&run(4).0).unwrap()
    );

    // With pairs that do differ, the same evidence reaches a decision, and a level record does not.
    let wins: Vec<GameResult> = (0..24)
        .map(|id| fake(id, "m", (id / 2) as u64, 7, 3))
        .collect();
    let s = ab::analyse("x", "y", &wins, Some(sprt), Vec::new())
        .sprt
        .unwrap();
    assert_eq!(s.decision, SprtDecision::AcceptH1, "{s:?}");
    assert!(s.llr >= s.upper && s.pairs_used == 12);
    let losses: Vec<GameResult> = (0..24)
        .map(|id| fake(id, "m", (id / 2) as u64, 3, 7))
        .collect();
    assert_eq!(
        ab::analyse("x", "y", &losses, Some(sprt), Vec::new())
            .sprt
            .unwrap()
            .decision,
        SprtDecision::AcceptH0
    );
    let level: Vec<GameResult> = (0..8)
        .map(|id| fake(id, "m", (id / 2) as u64, if id % 2 == 0 { 7 } else { 3 }, 5))
        .collect();
    assert_eq!(
        ab::analyse("x", "y", &level, Some(sprt), Vec::new())
            .sprt
            .unwrap()
            .decision,
        SprtDecision::Continue
    );
}

/// Every key path in a JSON value; keys under the listed dynamic maps collapse to `*`.
fn shape(v: &Value, path: &str, out: &mut BTreeSet<String>) {
    const DYNAMIC: [&str; 3] = ["metrics", "loadouts.attack", "loadouts.defend"];
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                let key = if DYNAMIC.contains(&path) {
                    "*"
                } else {
                    k.as_str()
                };
                let p = if path.is_empty() {
                    key.to_string()
                } else {
                    format!("{path}.{key}")
                };
                out.insert(p.clone());
                shape(child, &p, out);
            }
        }
        Value::Array(items) => items
            .iter()
            .for_each(|child| shape(child, &format!("{path}[]"), out)),
        _ => {}
    }
}

fn shape_of<T: serde::Serialize>(report: &T) -> String {
    let mut out = BTreeSet::new();
    shape(&serde_json::to_value(report).unwrap(), "", &mut out);
    out.into_iter().collect::<Vec<_>>().join("\n")
}

fn assert_snapshot(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(name);
    if std::env::var_os("IBC_BENCH_UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("{actual}\n")).unwrap();
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert_eq!(
        actual.trim(),
        expected.trim(),
        "the JSON schema of {name} changed. If that is intended, bump the report's `schema` string and re-run \
         with IBC_BENCH_UPDATE_SNAPSHOTS=1"
    );
}

fn small_league() -> (Vec<String>, Vec<GameResult>) {
    let pool = bots(["rusher", "holder"]);
    let run = league::run_league(&pool, &maps(), &[1, 2], true, &opts(2, true)).unwrap();
    assert!(run.results.iter().all(|r| r.ok()));
    (pool.iter().map(|b| b.label.clone()).collect(), run.results)
}

#[test]
fn report_json_schemas_match_their_snapshots() {
    let maps = maps();
    // `holder` loses most games to `rusher`, so `lost_games` has rows whatever the rules of the day are:
    // an empty list would drop its fields from the shape.
    let (ab_report, results) = ab::run_ab(
        &bots(["holder", "rusher"]),
        &maps,
        &[1, 2],
        Some(Sprt {
            elo0: 0.0,
            elo1: 20.0,
            alpha: 0.05,
            beta: 0.05,
        }),
        8,
        &opts(2, false),
    )
    .unwrap();
    assert_eq!(ab_report.schema, "ibc-bench/ab/1");
    assert_snapshot("ab.schema.txt", &shape_of(&ab_report));
    assert_snapshot(
        "noise.schema.txt",
        &shape_of(&noise::build("holder", "rusher", &results, Vec::new())),
    );

    let (labels, games) = small_league();
    let league_report = league::table(&labels, &games, Vec::new());
    assert_eq!(league_report.schema, "ibc-bench/league/1");
    assert_snapshot("league.schema.txt", &shape_of(&league_report));
    let balance_report = balance::build(&games, Vec::new());
    assert_eq!(balance_report.schema, "ibc-bench/balance/1");
    assert_snapshot("balance.schema.txt", &shape_of(&balance_report));
    // The report survives a round trip through its file format, which is what `--compare` reads.
    let text = report::to_json(&balance_report).unwrap();
    // (Floats may come back one ulp off, so the check is "nothing differs as far as compare can tell".)
    let loaded: balance::BalanceReport = serde_json::from_str(&text).unwrap();
    let diff = compare::compare(&balance_report, &loaded);
    assert!(
        diff.changes.is_empty() && diff.within_noise == 0,
        "{diff:?}"
    );
    assert_eq!(diff.identical, balance_report.metrics.len() as u64);
    assert_eq!(
        loaded.economy_by_round.len(),
        balance_report.economy_by_round.len()
    );
}

#[test]
fn details_agree_with_the_engine_summary() {
    let (_, games) = small_league();
    for g in &games {
        let d = g.detail.as_ref().expect("native games carry details");
        assert_eq!(
            d.rounds.len(),
            g.rounds.len(),
            "{} vs {} on {}",
            g.a,
            g.b,
            g.map
        );
        let label = format!("{} vs {} on {} seed {}", g.a, g.b, g.map, g.seed);
        assert_eq!(
            d.kills.len() as i64,
            g.totals[0].kills + g.totals[1].kills,
            "kills, {label}"
        );
        assert_eq!(
            d.shots_by_weapon.iter().sum::<i64>(),
            g.totals[0].shots + g.totals[1].shots,
            "shots, {label}"
        );
        assert_eq!(
            d.rounds.iter().map(|r| r.kills).sum::<i64>(),
            d.kills.len() as i64,
            "{label}"
        );
        for (r, log) in d.rounds.iter().zip(&g.rounds) {
            assert_eq!(r.round, log.round);
            assert!(r.loadout[0].len() == r.loadout[1].len() && !r.loadout[0].is_empty());
            assert!(
                r.first_contact.unwrap_or(0) <= r.first_kill.unwrap_or(i32::MAX),
                "{label}: kill before contact"
            );
            assert!(
                r.action_ticks < log.ticks,
                "{label}: action ticks exclude the buy phase"
            );
        }
    }
    let report = balance::build(&games, Vec::new());
    let rate = report.metrics["attacker_round_win_rate"];
    assert!(rate.value > 0.0 && rate.value < 1.0 && rate.lo.is_some());
    let reasons: f64 = report
        .metrics
        .iter()
        .filter(|(k, _)| k.starts_with("end_reason."))
        .map(|(_, e)| e.value)
        .sum();
    assert!(
        (reasons - 1.0).abs() < 1e-9,
        "round end reasons must partition the rounds, got {reasons}"
    );
    let bands: f64 = report
        .metrics
        .iter()
        .filter(|(k, _)| k.starts_with("kills.distance."))
        .map(|(_, e)| e.value)
        .sum();
    assert!((bands - 1.0).abs() < 1e-9);
    assert_eq!(
        report.rounds,
        games.iter().map(|g| g.rounds.len() as u64).sum::<u64>()
    );
}

#[test]
fn fuel_is_metered_by_default_and_unmetered_reports_say_so() {
    assert_eq!(
        RunOpts::new(PathBuf::from("unused")).backend,
        BackendKind::Wasm,
        "reported results must come from fuel-metered play unless asked otherwise"
    );
    assert!(BackendKind::parse("auto").is_err());
    assert_eq!(BackendKind::parse("native"), Ok(BackendKind::Native));

    // Every report and its stdout summary name the backend; native ones carry the UNMETERED note.
    let (labels, games) = small_league();
    assert!(games.iter().all(|g| g.backend == "native"));
    let league_text = league::render_markdown(&league::table(&labels, &games, Vec::new()));
    let balance_report = balance::build(&games, Vec::new());
    let pairs: Vec<GameResult> = (0..8)
        .map(|id| fake(id, "m", (id / 2) as u64, 7, 3))
        .collect();
    let ab_report = ab::analyse("x", "y", &pairs, None, Vec::new());
    let noise_report = noise::build("x", "y", &pairs, Vec::new());
    let comparison = compare::render(&compare::compare(&balance_report, &balance_report));
    for (what, text) in [
        ("league", league_text),
        ("balance.md", balance::render_markdown(&balance_report)),
        ("balance summary", balance::render_summary(&balance_report)),
        ("ab.md", ab::render_markdown(&ab_report)),
        ("ab summary", ab::render_summary(&ab_report)),
        ("noise", noise::render(&noise_report, false)),
        ("compare", comparison),
    ] {
        assert!(
            text.contains("native, UNMETERED (unlimited fuel)"),
            "{what} does not say it is unmetered:\n{text}"
        );
    }
    assert!(ab_report
        .warnings
        .iter()
        .any(|w| w.starts_with("UNMETERED")));

    // The same games played as wasm are metered and say that instead.
    let wasm: Vec<GameResult> = pairs
        .into_iter()
        .map(|mut g| {
            g.backend = "wasm".into();
            g
        })
        .collect();
    let ab_report = ab::analyse("x", "y", &wasm, None, Vec::new());
    let text = ab::render_markdown(&ab_report) + &ab::render_summary(&ab_report);
    assert!(!text.contains("UNMETERED"), "{text}");
    assert!(text.contains("wasm, fuel metered"), "{text}");
}

#[test]
fn compare_is_quiet_on_identical_reports_and_flags_a_changed_metric() {
    let (_, games) = small_league();
    let old = balance::build(&games, Vec::new());
    let same = compare::compare(&old, &old);
    assert!(same.changes.is_empty(), "{:?}", same.changes);
    assert_eq!(
        (same.identical, same.within_noise),
        (old.metrics.len() as u64, 0)
    );
    assert!(compare::render(&same).contains("no metric moved"));

    let mut new = old.clone();
    new.ruleset_hash = "feedfacefeedface".into();
    // A deliberate, large change: far outside the interval.
    let moved = new.metrics.get_mut("round_ticks.mean").unwrap();
    let shift = 10.0 * moved.half_width().unwrap().max(1.0);
    moved.value += shift;
    moved.lo = moved.lo.map(|v| v + shift);
    moved.hi = moved.hi.map(|v| v + shift);
    // A small wobble inside the interval must stay quiet.
    let wobble = new.metrics.get_mut("attacker_round_win_rate").unwrap();
    assert!(
        wobble.half_width().unwrap() > 0.002,
        "test needs a non-trivial interval"
    );
    wobble.value += 0.001;
    let c = compare::compare(&old, &new);
    let keys: Vec<(&str, compare::ChangeKind)> =
        c.changes.iter().map(|x| (x.key.as_str(), x.kind)).collect();
    assert_eq!(keys, vec![("round_ticks.mean", compare::ChangeKind::Moved)]);
    assert_eq!(c.within_noise, 1);
    let text = compare::render(&c);
    assert!(
        text.contains("MOVED beyond the noise (1)") && text.contains("round_ticks.mean"),
        "{text}"
    );
    assert!(text.contains("-> feedfacefeedface"));
}

#[test]
fn recorded_native_replay_verifies_and_explains_itself() {
    let map = &maps()[0];
    let replay =
        record_native(&map.text, 3, "rusher", "holder", ibc_sim::DEFAULT_MAX_TICKS).unwrap();
    ibc_replay::verify(&replay).expect("recorded replay re-simulates");
    let sim = ibc_sim::run(&map.text, 3, "rusher", "holder", ibc_sim::DEFAULT_MAX_TICKS).unwrap();
    assert_eq!(
        (replay.header.result.score, replay.header.result.ticks),
        (sim.score, sim.ticks)
    );
    assert_eq!(
        replay.ticks.last().unwrap().state_hash(),
        sim.final_hash,
        "recording loop drifted from ibc_sim::run_custom"
    );
    let decoded = ibc_replay::Replay::decode(&replay.encode()).unwrap();

    let d = dump::dump(&decoded).unwrap();
    assert_eq!(d.ticks_jsonl.lines().count(), replay.ticks.len() + 1);
    let first: Value = serde_json::from_str(d.ticks_jsonl.lines().nth(10).unwrap()).unwrap();
    assert_eq!(
        first["u"].as_array().unwrap().len(),
        2 * map
            .text
            .lines()
            .find_map(|l| l.strip_prefix("team_size:"))
            .unwrap()
            .trim()
            .parse::<usize>()
            .unwrap()
    );
    let rounds = (sim.score[0] + sim.score[1]) as usize;
    assert_eq!(d.rounds_csv.lines().count(), rounds + 1);

    let text = dump::why(&decoded, 1, 0, false).unwrap();
    assert!(
        text.contains("round 1") && text.contains("ROUND OVER"),
        "{text}"
    );
    assert!(
        text.contains("attacking"),
        "team A attacks in round 1:\n{text}"
    );
    assert!(dump::why(&decoded, 1, 0, true).unwrap().lines().count() > text.lines().count());
    assert!(dump::why(&decoded, 99, 0, false).is_err());
    assert!(dump::why(&decoded, 1, 99, false).is_err());
}

/// Opt-in (needs the `ibc` runner and a wasm toolchain): `IBC_BIN=/path/to/ibc cargo test -p ibc-bench -- --ignored`.
#[test]
#[ignore]
fn native_and_wasm_backends_agree() {
    let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let pool = Bot::parse_pool(
        &["rusher".to_string(), "holder".to_string()],
        &root.join("bots"),
    )
    .unwrap();
    let maps = maps();
    let schedule = paired_schedule(maps.len(), &[1]);
    let native = run_games(&pool, &maps, &schedule, &opts(2, true)).unwrap();
    let mut o = opts(2, true);
    o.backend = BackendKind::Wasm;
    o.out_dir = root.join("out/test-wasm-backend");
    let wasm = run_games(&pool, &maps, &schedule, &o).unwrap();
    assert!(wasm.warnings.is_empty(), "{:?}", wasm.warnings);
    for (n, w) in native.results.iter().zip(&wasm.results) {
        assert!(w.ok(), "{:?}", w.error);
        assert_eq!(
            (n.final_hash.as_str(), n.score, n.winner),
            (w.final_hash.as_str(), w.score, w.winner)
        );
        assert_eq!(
            n.detail, w.detail,
            "the collector must see the same match on both backends"
        );
    }
}
