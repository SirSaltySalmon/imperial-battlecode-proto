//! `ibc-maptool`: lint, analyze, render and transform `.ibcmap` files. See `docs/maps.md`.

use ibc_maptool::analyze::analyze;
use ibc_maptool::bands::Bands;
use ibc_maptool::lint::{fails, lint, Severity};
use ibc_maptool::raw::RawMap;
use ibc_maptool::render::{ascii, svg, Overlay};
use ibc_maptool::transform::{apply, mirror, scaffold, Axis, Op};
use ibc_maptool::{report, Map, Ruleset};
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage:
  ibc-maptool lint <map>... [--strict] [--rules <dir>]
  ibc-maptool analyze <map> [--format md|json] [--json <file>] [--bands <file.toml>] [--rules <dir>]
  ibc-maptool render <map> [-o <file.svg>] [--overlay <list>] [--rules <dir>]
  ibc-maptool ascii <map> [--overlay time-a|time-d|advantage] [--rules <dir>]
  ibc-maptool transform <map> --op <op> [--name <name>] [-o <file>]
  ibc-maptool mirror <map> --axis h|v|point [--swap-sides] [--name <name>] [-o <file>]
  ibc-maptool new --size WxH --team-size N [--name <name>] [-o <file>]
  ibc-maptool bands
overlays: routes, chokes, sightlines, time-a, time-d, advantage (comma separated)
transforms: flip-h, flip-v, transpose, rot90, rot180, rot270, swap-sides
`lint` exits non-zero on any error (`--strict`: on any warning too). `--rules <dir>` reads
core.toml, weapons.toml, abilities.toml and economy.toml from a directory instead of the built-in ruleset.";

/// Flags that take no value.
const SWITCHES: [&str; 2] = ["--strict", "--swap-sides"];

struct Args {
    positional: Vec<String>,
    flags: Vec<(String, String)>,
}

impl Args {
    fn parse(raw: &[String]) -> Result<Args, String> {
        let mut args = Args {
            positional: Vec::new(),
            flags: Vec::new(),
        };
        let mut it = raw.iter();
        while let Some(a) = it.next() {
            if SWITCHES.contains(&a.as_str()) {
                args.flags.push((a.clone(), String::new()));
            } else if a.starts_with('-') && a.len() > 1 {
                let v = it.next().ok_or_else(|| format!("{a} needs a value"))?;
                args.flags.push((a.clone(), v.clone()));
            } else {
                args.positional.push(a.clone());
            }
        }
        Ok(args)
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.flags
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Reject flags the command does not know, so a typo is not silently ignored.
    fn allow(&self, known: &[&str]) -> Result<(), String> {
        match self
            .flags
            .iter()
            .find(|(k, _)| !known.contains(&k.as_str()))
        {
            Some((k, _)) => Err(format!("unknown option {k}\n{USAGE}")),
            None => Ok(()),
        }
    }

    fn one_map(&self) -> Result<&str, String> {
        match self.positional.as_slice() {
            [one] => Ok(one),
            _ => Err(format!("expected exactly one map file\n{USAGE}")),
        }
    }
}

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
}

fn rules(args: &Args) -> Result<Ruleset, String> {
    match args.get("--rules") {
        None => Ok(Ruleset::builtin()),
        Some(dir) => {
            let file = |name: &str| read(&format!("{dir}/{name}.toml"));
            Ruleset::from_toml(
                &file("core")?,
                &file("weapons")?,
                &file("abilities")?,
                &file("economy")?,
            )
        }
    }
}

fn load_map(path: &str) -> Result<Map, String> {
    Map::parse(&read(path)?).map_err(|e| format!("{path}: {e}"))
}

/// Write to `-o` if given, else stdout.
fn emit(args: &Args, text: &str) -> Result<(), String> {
    match args.get("-o") {
        Some(path) => std::fs::write(path, text).map_err(|e| format!("{path}: {e}")),
        None => {
            print!("{text}");
            Ok(())
        }
    }
}

fn cmd_lint(args: &Args) -> Result<bool, String> {
    args.allow(&["--strict", "--rules"])?;
    if args.positional.is_empty() {
        return Err(format!("lint needs at least one map file\n{USAGE}"));
    }
    let rules = rules(args)?;
    let mut failed = false;
    for path in &args.positional {
        let stem = Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned());
        let issues = lint(&read(path)?, stem.as_deref(), &rules);
        let bad = fails(&issues, args.has("--strict"));
        failed |= bad;
        println!(
            "{path}: {}",
            if bad {
                "FAIL"
            } else if issues.is_empty() {
                "ok"
            } else {
                "ok (with warnings)"
            }
        );
        for i in &issues {
            let level = if i.severity == Severity::Error {
                "error"
            } else {
                "warning"
            };
            println!("  {level}[{}]: {}", i.code, i.message);
        }
    }
    Ok(!failed)
}

fn cmd_analyze(args: &Args) -> Result<bool, String> {
    args.allow(&["--format", "--json", "--bands", "--rules"])?;
    let map = load_map(args.one_map()?)?;
    let bands = match args.get("--bands") {
        Some(path) => Bands::from_toml(&read(path)?)?,
        None => Bands::default(),
    };
    let a = analyze(&map, &rules(args)?, &bands);
    if let Some(path) = args.get("--json") {
        std::fs::write(path, report::json(&a)).map_err(|e| format!("{path}: {e}"))?;
    }
    match args.get("--format").unwrap_or("md") {
        "md" => print!("{}", report::markdown(&a)),
        "json" => println!("{}", report::json(&a)),
        other => return Err(format!("unknown format `{other}` (md or json)")),
    }
    Ok(true)
}

fn cmd_picture(args: &Args, as_svg: bool) -> Result<bool, String> {
    args.allow(&["-o", "--overlay", "--rules"])?;
    let map = load_map(args.one_map()?)?;
    let overlays = Overlay::parse_list(args.get("--overlay").unwrap_or(""))?;
    let analysis = (!overlays.is_empty())
        .then(|| rules(args).map(|r| analyze(&map, &r, &Bands::default())))
        .transpose()?;
    let text = if as_svg {
        svg(&map, analysis.as_ref(), &overlays)
    } else {
        ascii(&map, analysis.as_ref(), &overlays)
    };
    emit(args, &text)?;
    Ok(true)
}

fn raw_map(path: &str) -> Result<RawMap, String> {
    RawMap::parse(&read(path)?).map_err(|e| format!("{path}: {e}"))
}

fn finish_edit(args: &Args, mut raw: RawMap) -> Result<bool, String> {
    if let Some(name) = args.get("--name") {
        raw.set("name", name);
    }
    emit(args, &raw.to_text())?;
    Ok(true)
}

fn cmd_transform(args: &Args) -> Result<bool, String> {
    args.allow(&["--op", "--name", "-o"])?;
    let op = Op::parse(
        args.get("--op")
            .ok_or_else(|| format!("transform needs --op\n{USAGE}"))?,
    )?;
    finish_edit(args, apply(&raw_map(args.one_map()?)?, op)?)
}

fn cmd_mirror(args: &Args) -> Result<bool, String> {
    args.allow(&["--axis", "--swap-sides", "--name", "-o"])?;
    let axis = Axis::parse(
        args.get("--axis")
            .ok_or_else(|| format!("mirror needs --axis\n{USAGE}"))?,
    )?;
    finish_edit(
        args,
        mirror(&raw_map(args.one_map()?)?, axis, args.has("--swap-sides"))?,
    )
}

fn cmd_new(args: &Args) -> Result<bool, String> {
    args.allow(&["--size", "--team-size", "--name", "-o"])?;
    let size = args
        .get("--size")
        .ok_or_else(|| format!("new needs --size WxH\n{USAGE}"))?;
    let (w, h) = size
        .split_once(['x', 'X'])
        .and_then(|(w, h)| Some((w.parse::<usize>().ok()?, h.parse::<usize>().ok()?)))
        .ok_or_else(|| format!("--size: `{size}` is not WxH"))?;
    let team = args.get("--team-size").unwrap_or("5");
    let team: usize = team
        .parse()
        .map_err(|_| format!("--team-size: `{team}` is not a number"))?;
    emit(
        args,
        &scaffold(w, h, team, args.get("--name").unwrap_or("unnamed"))?.to_text(),
    )?;
    Ok(true)
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some((cmd, rest)) = argv.split_first() else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let result = Args::parse(rest).and_then(|args| match cmd.as_str() {
        "lint" => cmd_lint(&args),
        "analyze" => cmd_analyze(&args),
        "render" => cmd_picture(&args, true),
        "ascii" => cmd_picture(&args, false),
        "transform" => cmd_transform(&args),
        "mirror" => cmd_mirror(&args),
        "new" => cmd_new(&args),
        "bands" => {
            print!("{}", Bands::default().to_toml());
            Ok(true)
        }
        _ => Err(USAGE.to_string()),
    });
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
