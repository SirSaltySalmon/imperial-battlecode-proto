//! `ibc build --debug` and a bot that prints, end to end: a scratch bot crate built against the Rust SDK,
//! played in a real match with `ibc run`. Needs the wasm32-wasip1 Rust target; skipped (with a note)
//! where it is not installed.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn ibc(args: &[&dyn AsRef<OsStr>]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ibc"))
        .args(args)
        .env("IBC_NO_UPDATE_CHECK", "1")
        .current_dir(repo())
        .output()
        .unwrap()
}

fn ok(out: &Output) -> String {
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    text
}

/// Whether rustc has a standard library for wasm32-wasip1.
fn has_wasi_target() -> bool {
    let Ok(out) = Command::new("rustc")
        .args(["--print", "target-libdir", "--target", "wasm32-wasip1"])
        .output()
    else {
        return false;
    };
    let dir = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    out.status.success() && std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some())
}

/// Leave an unchanged file alone, so the bot's own cargo build stays incremental between runs.
fn write(path: &Path, text: &str) {
    if std::fs::read_to_string(path).ok().as_deref() != Some(text) {
        std::fs::write(path, text).unwrap();
    }
}

/// Prints on both streams every turn; on its third turn adds to i32::MAX, which panics in a debug build.
const MAIN: &str = r#"use ibc::Bot;

struct Printer(i32);

impl Bot for Printer {
    fn new() -> Self {
        Printer(0)
    }

    fn turn(&mut self) {
        self.0 += 1;
        println!("stdout {}", self.0);
        eprintln!("stderr {}", self.0);
        if self.0 == 3 {
            println!("sum {}", std::hint::black_box(i32::MAX) + self.0);
        }
    }
}

ibc::bot!(Printer);
"#;

#[test]
fn a_debug_build_prints_and_faults_on_overflow() {
    if !has_wasi_target() {
        eprintln!("skipped: no wasm32-wasip1 target (rustup target add wasm32-wasip1)");
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("debug-build");
    let bot = dir.join("printer");
    std::fs::create_dir_all(bot.join("src")).unwrap();
    let sdk = repo().join("sdk/rust").to_string_lossy().replace('\\', "/");
    let manifest = format!(
        "[package]\nname = \"printer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n\n\
         [dependencies]\nibc = {{ path = '{sdk}' }}\n"
    );
    write(&bot.join("Cargo.toml"), &manifest);
    write(&bot.join("src/main.rs"), MAIN);

    let built = ok(&ibc(&[&"build", &"--debug", &bot]));
    assert!(built.contains("language rust"), "{built}");
    let wasm = bot.join(".ibc/bot.wasm");
    let bytes = std::fs::read(&wasm).unwrap();
    assert!(
        bytes.windows(11).any(|w| w == b".debug_info"),
        "--debug keeps DWARF in the module"
    );

    let out = dir.join("match");
    let idle = repo().join("fixtures/golden/bots/idle.wasm");
    let map = repo().join("maps/crossfire.ibcmap");
    ok(&ibc(&[
        &"run",
        &wasm,
        &idle,
        &"--map",
        &map,
        &"--seed",
        &"1",
        &"--max-ticks",
        &"5",
        &"-o",
        &out,
    ]));

    let log = std::fs::read_to_string(out.join("team-a.log")).unwrap();
    let units: Vec<&str> = log.split("=== unit ").skip(1).collect();
    assert!(!units.is_empty(), "{log}");
    for unit in &units {
        // Both streams, in order, then the panic message from the debug build's overflow check.
        let printed = "stdout 1\nstderr 1\nstdout 2\nstderr 2\nstdout 3\nstderr 3\n";
        assert!(unit.contains(printed), "{unit}");
        assert!(unit.contains("attempt to add with overflow"), "{unit}");
        assert!(
            unit.contains("faulted at tick 2") && !unit.contains("sum "),
            "{unit}"
        );
    }
    let summary: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("summary.json")).unwrap()).unwrap();
    assert_eq!(
        summary["teams"][0]["faults"],
        units.len(),
        "every unit of team A faulted"
    );
    assert_eq!(summary["teams"][1]["faults"], 0);
    ok(&ibc(&[&"replay", &"verify", &out.join("replay.ibcr")]));
}
