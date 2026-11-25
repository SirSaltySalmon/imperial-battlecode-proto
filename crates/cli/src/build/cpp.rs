//! C++ bots: `src/**/*.cpp` compiled and linked by wasi-sdk's clang for `wasm32-wasip1`, in one direct
//! clang invocation (no CMake, no Ninja). The SDK headers are embedded in this binary and written to
//! `<bot_dir>/.ibc/include`, so a bot directory builds anywhere `ibc` and wasi-sdk exist.
//!
//! A bot may carry an `ibc.toml`:
//!
//! ```toml
//! [cpp]
//! exceptions = true   # default false: -fno-exceptions gives smaller, cheaper wasm
//! ```

use crate::toolchain;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const TARGET: &str = "wasm32-wasip1";
pub const CONFIG_FILE: &str = "ibc.toml";

/// The C++ SDK, shipped inside the CLI.
const HEADERS: &[(&str, &str)] = &[
    ("ibc.h", include_str!("../../../../sdk/cpp/ibc.h")),
    ("ibc_abi.h", include_str!("../../../../sdk/cpp/ibc_abi.h")),
];

/// Stack of a bot instance, the same as a Rust bot gets from rustc.
const STACK_BYTES: u32 = 1 << 20;

#[derive(Deserialize, Default, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
struct BotConfig {
    #[serde(default)]
    cpp: CppConfig,
}

#[derive(Deserialize, Default, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
struct CppConfig {
    /// Build with C++ exceptions (the wasm exception-handling proposal) instead of `-fno-exceptions`.
    #[serde(default)]
    exceptions: bool,
}

fn load_config(dir: &Path) -> Result<BotConfig> {
    let path = dir.join(CONFIG_FILE);
    if !path.is_file() {
        return Ok(BotConfig::default());
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Does `dir` hold C++ sources directly in `src/`? (Language detection; the build also takes subdirectories.)
pub fn is_cpp_bot(dir: &Path) -> bool {
    std::fs::read_dir(dir.join("src"))
        .map(|entries| {
            entries
                .flatten()
                .any(|e| is_cpp(&e.path()) && e.path().is_file())
        })
        .unwrap_or(false)
}

fn is_cpp(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "cpp")
}

/// Every `.cpp` under `dir/src`, as sorted `/`-separated paths relative to `dir` (so the command line,
/// and with it the output, does not depend on directory order or on where the bot lives).
fn sources(dir: &Path) -> Result<Vec<String>> {
    fn walk(at: &Path, rel: &str, out: &mut Vec<String>) -> Result<()> {
        for entry in std::fs::read_dir(at).with_context(|| format!("reading {}", at.display()))? {
            let entry = entry.with_context(|| format!("reading {}", at.display()))?;
            let (path, name) = (
                entry.path(),
                entry.file_name().to_string_lossy().into_owned(),
            );
            let rel = format!("{rel}/{name}");
            if path.is_dir() {
                walk(&path, &rel, out)?;
            } else if is_cpp(&path) {
                out.push(rel);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(&dir.join("src"), "src", &mut out)?;
    out.sort();
    Ok(out)
}

/// Write `content` unless the file already holds it (keeps mtimes quiet for editors and build caches).
fn write_if_changed(path: &Path, content: &[u8]) -> Result<()> {
    if std::fs::read(path).ok().as_deref() != Some(content) {
        std::fs::write(path, content).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

/// The clang arguments after the program name. Paths are relative to the bot directory.
fn clang_args(
    sysroot: &Path,
    sources: &[String],
    has_include_dir: bool,
    exceptions: bool,
    release: bool,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        format!("--target={TARGET}"),
        format!("--sysroot={}", sysroot.display()),
        "-std=c++20".into(),
        "-msimd128".into(),
        "-mbulk-memory".into(),
        "-Wall".into(),
        "-I.ibc/include".into(),
        "-Isrc".into(),
    ];
    if has_include_dir {
        args.push("-Iinclude".into());
    }
    if release {
        args.extend(["-O2".into(), "-DNDEBUG".into(), "-Wl,--strip-all".into()]);
    } else {
        args.extend(["-O0".into(), "-g".into()]);
    }
    if exceptions {
        // The standard (exnref) encoding, not the legacy one; clang then links the `eh` libc++ of the sysroot.
        args.extend([
            "-fwasm-exceptions".into(),
            "-mllvm".into(),
            "-wasm-use-legacy-eh=false".into(),
        ]);
    } else {
        args.push("-fno-exceptions".into());
    }
    args.extend(sources.iter().cloned());
    if exceptions {
        args.push("-lunwind".into());
    }
    args.push(format!("-Wl,-z,stack-size={STACK_BYTES}"));
    // Never let a wasm-opt that happens to be on PATH rewrite the module: fuel use must not depend on it.
    args.push("--no-wasm-opt".into());
    args.extend(["-o".into(), ".ibc/build/bot.wasm".into()]);
    args
}

/// Build the bot in `dir` and return the path of the linked wasm.
pub fn build(dir: &Path, release: bool) -> Result<PathBuf> {
    let config = load_config(dir)?;
    let sources = sources(dir)?;
    if sources.is_empty() {
        bail!("{}: no .cpp files under src/", dir.display());
    }
    let sdk = toolchain::locate().context("C++ bots are built with wasi-sdk")?;

    let include = dir.join(".ibc").join("include");
    let out_dir = dir.join(".ibc").join("build");
    for d in [&include, &out_dir] {
        std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    }
    for (name, content) in HEADERS {
        write_if_changed(&include.join(name), content.as_bytes())?;
    }

    let mut cmd = Command::new(sdk.clangxx());
    cmd.current_dir(dir).args(clang_args(
        &sdk.sysroot(),
        &sources,
        dir.join("include").is_dir(),
        config.cpp.exceptions,
        release,
    ));
    // The build is a function of the sources and the SDK: do not let the caller's environment add flags or paths.
    for var in [
        "CPATH",
        "C_INCLUDE_PATH",
        "CPLUS_INCLUDE_PATH",
        "LIBRARY_PATH",
        "CCC_OVERRIDE_OPTIONS",
        "CLANG_CONFIG_FILE_SYSTEM_DIR",
    ] {
        cmd.env_remove(var);
    }
    let status = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("running {}", sdk.clangxx().display()))?;
    if !status.success() {
        bail!("clang++ failed in {} ({status})", dir.display());
    }
    Ok(out_dir.join("bot.wasm"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const RUST_ABI: &str = include_str!("../../../abi/src/lib.rs");
    const CPP_ABI: &str = include_str!("../../../../sdk/cpp/ibc_abi.h");

    /// (type, value) with the value normalised: integers and `a << b` are evaluated, anything else is kept as text.
    fn value(rhs: &str) -> String {
        let int = |s: &str| s.trim().replace('_', "").parse::<i128>().ok();
        let rhs = rhs.trim();
        match rhs.split_once("<<") {
            Some((a, b)) => match (int(a), int(b)) {
                (Some(a), Some(b)) => (a << b).to_string(),
                _ => rhs.to_string(),
            },
            None => int(rhs).map_or(rhs.to_string(), |v| v.to_string()),
        }
    }

    /// `pub const NAME: T = V;` lines of the Rust crate, as name -> (C++ type, value).
    fn rust_consts() -> BTreeMap<String, (String, String)> {
        let mut out = BTreeMap::new();
        for line in RUST_ABI.lines() {
            let Some(rest) = line.strip_prefix("pub const ") else {
                continue;
            };
            let (name, rest) = rest.split_once(':').expect("const without a type");
            let (ty, rhs) = rest.split_once('=').expect("const without a value");
            let ty = match ty.trim() {
                "i32" => "int32_t",
                "u32" => "uint32_t",
                "u8" => "uint8_t",
                "u64" => "uint64_t",
                "&str" => "char[]",
                other => panic!("const {name}: teach this test (and ibc_abi.h) about type {other}"),
            };
            out.insert(
                name.to_string(),
                (ty.to_string(), value(rhs.trim_end_matches(';'))),
            );
        }
        out
    }

    /// `inline constexpr T NAME = V;` lines of the header.
    fn cpp_consts() -> BTreeMap<String, (String, String)> {
        let mut out = BTreeMap::new();
        for line in CPP_ABI.lines() {
            let Some(rest) = line.strip_prefix("inline constexpr ") else {
                continue;
            };
            let (decl, rhs) = rest.split_once('=').expect("constexpr without a value");
            let (ty, name) = decl
                .trim()
                .split_once(' ')
                .expect("constexpr without a name");
            let (ty, name) = match name.strip_suffix("[]") {
                Some(n) => (format!("{ty}[]"), n),
                None => (ty.to_string(), name),
            };
            let old = out.insert(name.to_string(), (ty, value(rhs.trim_end_matches(';'))));
            assert!(old.is_none(), "{name} is defined twice in ibc_abi.h");
        }
        out
    }

    /// Records as name -> flattened field list (`charges[2]` for arrays).
    fn rust_records() -> BTreeMap<String, Vec<String>> {
        let mut out = BTreeMap::new();
        let mut current: Option<(String, Vec<String>)> = None;
        for line in RUST_ABI.lines() {
            if let Some(rest) = line.strip_prefix("pub struct ") {
                current = Some((rest.trim_end_matches('{').trim().to_string(), Vec::new()));
            } else if line.starts_with('}') {
                if let Some((name, fields)) = current.take() {
                    out.insert(name, fields);
                }
            } else if let (Some((name, fields)), Some(rest)) =
                (current.as_mut(), line.trim().strip_prefix("pub "))
            {
                let (field, ty) = rest
                    .trim_end_matches(',')
                    .split_once(':')
                    .expect("field without a type");
                match ty.trim() {
                    "i32" => fields.push(field.to_string()),
                    arr => {
                        let n = arr.strip_prefix("[i32; ").and_then(|a| a.strip_suffix(']'));
                        let n = n.unwrap_or_else(|| {
                            panic!("{name}.{field}: records are made of i32 only, found {arr}")
                        });
                        fields.push(format!("{field}[{n}]"));
                    }
                }
            }
        }
        out
    }

    /// Header records as name -> (fields, the size its `static_assert` claims).
    fn cpp_records() -> BTreeMap<String, (Vec<String>, usize)> {
        let mut out: BTreeMap<String, (Vec<String>, usize)> = BTreeMap::new();
        let mut current: Option<(String, Vec<String>)> = None;
        for line in CPP_ABI.lines() {
            if let Some(rest) = line.strip_prefix("struct ") {
                current = Some((rest.trim_end_matches('{').trim().to_string(), Vec::new()));
            } else if line.starts_with("};") {
                if let Some((name, fields)) = current.take() {
                    out.insert(name, (fields, 0));
                }
            } else if let Some(rest) = line.strip_prefix("static_assert(sizeof(") {
                let (name, size) = rest
                    .split_once(") == ")
                    .expect("static_assert(sizeof(T) == N);");
                out.get_mut(name)
                    .expect("static_assert before its struct")
                    .1 = size.trim_end_matches(");").parse().unwrap();
            } else if let Some((_, fields)) = current.as_mut() {
                let line = line.trim();
                if line.is_empty() || line.starts_with("//") {
                    continue;
                }
                let field = line
                    .strip_prefix("int32_t ")
                    .and_then(|f| f.strip_suffix(';'));
                fields.push(
                    field
                        .unwrap_or_else(|| {
                            panic!("record fields must be `int32_t name;`, found `{line}`")
                        })
                        .to_string(),
                );
            }
        }
        out
    }

    fn i32s(fields: &[String]) -> usize {
        fields
            .iter()
            .map(|f| {
                f.split_once('[')
                    .map_or(1, |(_, n)| n.trim_end_matches(']').parse().unwrap())
            })
            .sum()
    }

    #[test]
    fn abi_header_constants_match_the_rust_crate() {
        let (rust, cpp) = (rust_consts(), cpp_consts());
        assert!(rust.len() > 100, "the parser lost the Rust constants");
        for (name, want) in &rust {
            assert_eq!(
                cpp.get(name),
                Some(want),
                "sdk/cpp/ibc_abi.h: {name} is missing or differs from crates/abi"
            );
        }
        for name in cpp.keys() {
            assert!(
                rust.contains_key(name),
                "sdk/cpp/ibc_abi.h: {name} does not exist in crates/abi"
            );
        }
        assert_eq!(cpp["TILE"].1, ibc_abi::TILE.to_string());
        assert_eq!(cpp["S_REVEALED"].1, ibc_abi::S_REVEALED.to_string());
    }

    #[test]
    fn abi_header_records_match_the_rust_crate() {
        use ibc_abi::*;
        use std::mem::size_of;
        macro_rules! sizes { ($($t:ident),*) => { BTreeMap::from([$((stringify!($t), size_of::<$t>())),*]) } }
        let real = sizes!(
            UnitInfo,
            SoundInfo,
            ProjectileInfo,
            EffectInfo,
            MatchInfo,
            MapInfo,
            SiteInfo,
            DropInfo,
            EventInfo,
            CoreRules,
            WeaponRules,
            AbilityRules,
            ItemRules,
            KitRules
        );
        let (rust, cpp) = (rust_records(), cpp_records());
        assert_eq!(
            rust.keys().map(String::as_str).collect::<Vec<_>>(),
            real.keys().copied().collect::<Vec<_>>()
        );
        assert_eq!(
            cpp.keys().collect::<Vec<_>>(),
            rust.keys().collect::<Vec<_>>(),
            "record set differs"
        );
        for (name, fields) in &rust {
            let (cpp_fields, asserted) = &cpp[name];
            assert_eq!(
                cpp_fields, fields,
                "sdk/cpp/ibc_abi.h: fields of {name} differ from crates/abi"
            );
            assert_eq!(
                real[name.as_str()],
                4 * i32s(fields),
                "{name}: the Rust parser miscounted"
            );
            assert_eq!(
                *asserted,
                real[name.as_str()],
                "sdk/cpp/ibc_abi.h: static_assert(sizeof({name})) is wrong"
            );
        }
    }

    #[test]
    fn abi_header_declares_every_host_import() {
        for name in ibc_runner::IBC_IMPORTS {
            let decl = format!("IBC_IMPORT({name})");
            assert_eq!(
                CPP_ABI.matches(&decl).count(),
                1,
                "sdk/cpp/ibc_abi.h must declare {decl} exactly once"
            );
        }
        assert_eq!(
            CPP_ABI.matches("IBC_IMPORT(").count() - 1,
            ibc_runner::IBC_IMPORTS.len(),
            "unknown import declared"
        );
        assert!(CPP_ABI.contains(&format!("import_module(\"{}\")", ibc_abi::IMPORT_MODULE)));
    }

    #[test]
    fn config_defaults_and_rejects_unknown_keys() {
        assert_eq!(
            toml::from_str::<BotConfig>("").unwrap(),
            BotConfig::default()
        );
        assert!(
            toml::from_str::<BotConfig>("[cpp]\nexceptions = true\n")
                .unwrap()
                .cpp
                .exceptions
        );
        assert!(toml::from_str::<BotConfig>("[cpp]\nflags = [\"-O3\"]\n").is_err());
    }

    #[test]
    fn exceptions_are_opt_in() {
        let src = vec!["src/main.cpp".to_string()];
        let plain = clang_args(Path::new("/sdk/sysroot"), &src, false, false, true);
        assert!(
            plain.iter().any(|a| a == "-fno-exceptions") && !plain.iter().any(|a| a == "-lunwind")
        );
        assert!([
            "-O2",
            "-std=c++20",
            "-msimd128",
            "-mbulk-memory",
            "--no-wasm-opt"
        ]
        .iter()
        .all(|f| plain.iter().any(|a| a == f)));
        let eh = clang_args(Path::new("/sdk/sysroot"), &src, false, true, true);
        assert!(!eh.iter().any(|a| a == "-fno-exceptions"));
        assert!(
            ["-fwasm-exceptions", "-wasm-use-legacy-eh=false", "-lunwind"]
                .iter()
                .all(|f| eh.iter().any(|a| a == f))
        );
    }

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// Needs wasi-sdk: skipped (with a note) where `ibc toolchain status` would fail.
    #[test]
    fn starter_builds_and_geometry_matches_rust_when_toolchain_present() {
        let Ok(sdk) = toolchain::locate() else {
            eprintln!("skipped: no wasi-sdk (set IBC_WASI_SDK or run `ibc toolchain install`)");
            return;
        };
        // The golden values in geom_test.cpp are static_asserts: compiling it is the test.
        let status = Command::new(sdk.clangxx())
            .current_dir(repo().join("sdk/cpp"))
            .args([
                &format!("--target={TARGET}"),
                "-std=c++20",
                "-fsyntax-only",
                "-Wall",
                "-Werror",
                "-I.",
                "tests/geom_test.cpp",
            ])
            .arg(format!("--sysroot={}", sdk.sysroot().display()))
            .status()
            .unwrap();
        assert!(
            status.success(),
            "sdk/cpp/tests/geom_test.cpp failed to compile"
        );

        let starter = repo().join("bots/cpp-starter");
        assert!(is_cpp_bot(&starter));
        let wasm = std::fs::read(build(&starter, true).unwrap()).unwrap();
        ibc_runner::check_wasm(&wasm).unwrap();
        assert!(
            wasm.len() < 64 << 10,
            "cpp-starter grew to {} bytes: did something pull in iostream or printf?",
            wasm.len()
        );
    }
}
