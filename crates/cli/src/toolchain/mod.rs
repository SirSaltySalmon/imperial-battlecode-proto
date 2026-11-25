//! Toolchains `ibc build` needs beyond cargo. Today that is wasi-sdk (clang for `wasm32-wasip1`) for C++ bots.
//!
//! Lookup order: the `IBC_WASI_SDK` environment variable (a wasi-sdk root), else the copy that
//! `ibc toolchain install` unpacks into the per-user cache directory.

mod install;

pub use install::install;

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The wasi-sdk release every C++ bot is built with. Part of what makes a build reproducible.
pub const WASI_SDK_MAJOR: &str = "33";
pub const WASI_SDK_VERSION: &str = "33.0";
/// Overrides the lookup with an existing wasi-sdk root.
pub const ENV_WASI_SDK: &str = "IBC_WASI_SDK";

const EXE: &str = std::env::consts::EXE_SUFFIX;

/// An unpacked wasi-sdk.
#[derive(Clone, Debug)]
pub struct WasiSdk {
    pub root: PathBuf,
}

impl WasiSdk {
    /// `root` if it looks like a wasi-sdk (has clang++ and the sysroot), else an error saying what is missing.
    pub fn at(root: &Path) -> Result<WasiSdk> {
        let sdk = WasiSdk {
            root: root.to_path_buf(),
        };
        for needed in [sdk.clangxx(), sdk.sysroot()] {
            if !needed.exists() {
                bail!(
                    "{} is not a wasi-sdk: {} is missing",
                    root.display(),
                    needed.display()
                );
            }
        }
        Ok(sdk)
    }

    pub fn clangxx(&self) -> PathBuf {
        self.root.join("bin").join(format!("clang++{EXE}"))
    }

    pub fn sysroot(&self) -> PathBuf {
        self.root.join("share").join("wasi-sysroot")
    }

    /// First line of the SDK's `VERSION` file, e.g. `33.0`.
    pub fn version(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.root.join("VERSION")).ok()?;
        text.lines().next().map(|l| l.trim().to_string())
    }
}

/// The per-user cache directory of `ibc` (not created here).
pub fn cache_dir() -> Result<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(windows) {
        var("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library").join("Caches"))
    } else {
        var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|h| h.join(".cache")))
    };
    match base {
        Some(b) => Ok(b.join("ibc")),
        None => bail!("cannot find a per-user cache directory (set {ENV_WASI_SDK} to a wasi-sdk root instead)"),
    }
}

/// Where `ibc toolchain install` puts wasi-sdk.
pub fn installed_path() -> Result<PathBuf> {
    Ok(cache_dir()?.join(format!("wasi-sdk-{WASI_SDK_MAJOR}")))
}

/// Find wasi-sdk, or explain how to get it.
pub fn locate() -> Result<WasiSdk> {
    if let Some(root) = std::env::var_os(ENV_WASI_SDK).filter(|v| !v.is_empty()) {
        return WasiSdk::at(Path::new(&root)).with_context(|| format!("{ENV_WASI_SDK} is set"));
    }
    let path = installed_path()?;
    if !path.exists() {
        bail!(
            "wasi-sdk {WASI_SDK_VERSION} is not installed (looked in {}). Run `ibc toolchain install`, or set {ENV_WASI_SDK} to a wasi-sdk root",
            path.display()
        );
    }
    WasiSdk::at(&path)
}

/// `ibc toolchain status`: what is pinned, where it is looked up, and whether it works.
pub fn status() -> Result<()> {
    println!("wasi-sdk pinned {WASI_SDK_VERSION}");
    match std::env::var_os(ENV_WASI_SDK).filter(|v| !v.is_empty()) {
        Some(v) => println!("{ENV_WASI_SDK} {}", Path::new(&v).display()),
        None => println!("{ENV_WASI_SDK} (not set)"),
    }
    match installed_path() {
        Ok(p) => println!("cache {}", p.display()),
        Err(e) => println!("cache unavailable: {e}"),
    }
    let sdk = match locate() {
        Ok(sdk) => sdk,
        Err(e) => {
            println!("status missing: {e:#}");
            std::process::exit(1);
        }
    };
    println!("root {}", sdk.root.display());
    let version = sdk.version().unwrap_or_else(|| "unknown".into());
    println!("version {version}");
    if !version.starts_with(WASI_SDK_VERSION) {
        println!("warning: builds are only reproducible with wasi-sdk {WASI_SDK_VERSION}");
    }
    let out = Command::new(sdk.clangxx())
        .arg("--version")
        .output()
        .with_context(|| format!("running {}", sdk.clangxx().display()))?;
    if !out.status.success() {
        bail!(
            "{} --version failed ({})",
            sdk.clangxx().display(),
            out.status
        );
    }
    println!(
        "clang {}",
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
    );
    println!("status ok");
    Ok(())
}
