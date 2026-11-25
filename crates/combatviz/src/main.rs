//! `ibc-combatviz [--out path]` writes the combat-lab recording the viewer draws.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut out = PathBuf::from("out/combat-lab/lab.json");
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!(
                    "ibc-combatviz [--out PATH]\n\n\
                     Record the combat tests (weapon fans, time-to-kill duels, the firing and ballistics\n\
                     scripts). Writes JSON (default out/combat-lab/lab.json).\n\n\
                     The replay viewer draws it. From viewer/: npm run dev, then open /lab.html\n\
                     and drop the file, or pass --out viewer/public/lab.json so /lab.html loads it."
                );
                return Ok(());
            }
            "-o" | "--out" => {
                out = PathBuf::from(args.next().ok_or("--out needs a path")?);
            }
            other => return Err(format!("unknown argument {other} (try --help)")),
        }
    }
    let lab = ibc_combatviz::build();
    let scenes = lab.scenes.len();
    let ruleset = lab.ruleset.clone();
    let json = serde_json::to_string(&lab).map_err(|err| err.to_string())?;
    if let Some(dir) = out.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
        }
    }
    std::fs::write(&out, json).map_err(|err| err.to_string())?;
    println!(
        "wrote {} ({} scenes, ruleset {ruleset})",
        out.display(),
        scenes
    );
    Ok(())
}
