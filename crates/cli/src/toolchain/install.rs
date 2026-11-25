//! `ibc toolchain install`: download the pinned wasi-sdk release, verify its sha256, unpack it.
//!
//! Downloading and unpacking are delegated to the system `curl` and `tar` (both ship with Windows 10+,
//! macOS and every Linux we target), so the CLI carries no HTTP or archive code.

use super::{installed_path, WasiSdk, WASI_SDK_MAJOR, WASI_SDK_VERSION};
use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// One official release archive: `wasi-sdk-<version>-<platform>.tar.gz`.
struct Pin {
    os: &'static str,
    arch: &'static str,
    /// The `<arch>-<os>` part of the asset name.
    platform: &'static str,
    sha256: &'static str,
}

/// Assets of https://github.com/WebAssembly/wasi-sdk/releases/tag/wasi-sdk-33 with the sha256 digests
/// GitHub publishes for them. Bump together with `WASI_SDK_VERSION`.
const PINS: &[Pin] = &[
    Pin {
        os: "windows",
        arch: "x86_64",
        platform: "x86_64-windows",
        sha256: "df14ca2a2127c2d6b6be07e6f5549b3af9c1b3c0112430c200a4749970c59f06",
    },
    Pin {
        os: "windows",
        arch: "aarch64",
        platform: "arm64-windows",
        sha256: "2f457a62da1ce1a55e2ba77c450401b3551f27f04f0a87112b74c5aa8dd9504f",
    },
    Pin {
        os: "linux",
        arch: "x86_64",
        platform: "x86_64-linux",
        sha256: "0ba8b5bfaeb2adf3f29bab5841d76cf5318ab8e1642ea195f88baba1abd47bce",
    },
    Pin {
        os: "linux",
        arch: "aarch64",
        platform: "arm64-linux",
        sha256: "4f98ee738c7abb45c81a94d1461fc53cc569d1cd01498951c8184d841a027844",
    },
    Pin {
        os: "linux",
        arch: "riscv64",
        platform: "riscv64-linux",
        sha256: "f0399061bbedb918bc32f0c311bb3a140f5eafc4ea8cdd0d5fc18f283a42d8b3",
    },
    Pin {
        os: "macos",
        arch: "x86_64",
        platform: "x86_64-macos",
        sha256: "18f3f201ba9734e6a4455b0b6410690395a55e9ffa9f6f5066f66083a94b93b3",
    },
    Pin {
        os: "macos",
        arch: "aarch64",
        platform: "arm64-macos",
        sha256: "85c997a2665ead91673b5bb88b7d0df3fc8900df3bfa244f720d478187bbdc78",
    },
];

fn pin_for(os: &str, arch: &str) -> Result<&'static Pin> {
    PINS.iter()
        .find(|p| p.os == os && p.arch == arch)
        .ok_or_else(|| anyhow!("no wasi-sdk {WASI_SDK_VERSION} release for {arch}-{os}; build it yourself and set IBC_WASI_SDK"))
}

fn asset_name(pin: &Pin) -> String {
    format!("wasi-sdk-{WASI_SDK_VERSION}-{}.tar.gz", pin.platform)
}

fn asset_url(pin: &Pin) -> String {
    format!(
        "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-{WASI_SDK_MAJOR}/{}",
        asset_name(pin)
    )
}

/// sha256 of a file, streamed (the Windows archive is over 600 MB).
fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("reading {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// The system tar. On Windows the bsdtar in System32 is used on purpose: a GNU tar found on PATH
/// (Git Bash, MSYS) reads `C:\...` as a remote host.
fn tar_program() -> PathBuf {
    if cfg!(windows) {
        if let Some(root) = std::env::var_os("SystemRoot") {
            let tar = Path::new(&root).join("System32").join("tar.exe");
            if tar.is_file() {
                return tar;
            }
        }
    }
    PathBuf::from("tar")
}

fn run(cmd: &mut Command, what: &str) -> Result<()> {
    let status = cmd
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("{what}: could not start {:?}", cmd.get_program()))?;
    if !status.success() {
        bail!("{what} failed ({status})");
    }
    Ok(())
}

fn download(url: &str, to: &Path) -> Result<()> {
    let mut cmd = Command::new("curl");
    cmd.args([
        "--fail",
        "--location",
        "--proto",
        "=https",
        "--retry",
        "3",
        "--continue-at",
        "-",
        "--output",
    ])
    .arg(to)
    .arg(url);
    run(&mut cmd, "download with curl")
}

/// Check `archive` against `sha256`, unpack it (dropping the archive's single top-level directory)
/// and move the result to `dest`, replacing what is there. `dest` only ever holds a complete SDK.
fn unpack_verified(archive: &Path, sha256: &str, dest: &Path) -> Result<WasiSdk> {
    let actual = sha256_file(archive)?;
    if actual != sha256 {
        bail!(
            "{}: sha256 mismatch\n  expected {sha256}\n  actual   {actual}\nrefusing to unpack it",
            archive.display()
        );
    }
    let staging = dest.with_extension("unpack");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .with_context(|| format!("removing {}", staging.display()))?;
    }
    std::fs::create_dir_all(&staging).with_context(|| format!("creating {}", staging.display()))?;
    let mut tar = Command::new(tar_program());
    tar.arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(&staging)
        .arg("--strip-components=1");
    run(&mut tar, "unpack with tar")?;
    WasiSdk::at(&staging).context("the unpacked archive")?;
    if dest.exists() {
        std::fs::remove_dir_all(dest).with_context(|| format!("removing {}", dest.display()))?;
    }
    std::fs::rename(&staging, dest)
        .with_context(|| format!("moving {} to {}", staging.display(), dest.display()))?;
    WasiSdk::at(dest)
}

/// Install wasi-sdk into the per-user cache. `archive` is an already downloaded release archive
/// (still verified against the pin); without it the archive is downloaded.
pub fn install(force: bool, archive: Option<&Path>) -> Result<()> {
    let pin = pin_for(std::env::consts::OS, std::env::consts::ARCH)?;
    let dest = installed_path()?;
    let cache = dest
        .parent()
        .ok_or_else(|| anyhow!("bad cache path {}", dest.display()))?;
    std::fs::create_dir_all(cache).with_context(|| format!("creating {}", cache.display()))?;

    // One install at a time per user. The OS drops the lock if the process dies, so it cannot go stale.
    let lock_path = dest.with_extension("lock");
    let lock =
        File::create(&lock_path).with_context(|| format!("creating {}", lock_path.display()))?;
    if lock.try_lock().is_err() {
        eprintln!("another `ibc toolchain install` is running; waiting for it");
        lock.lock()
            .with_context(|| format!("locking {}", lock_path.display()))?;
    }

    if !force {
        if let Ok(sdk) = WasiSdk::at(&dest) {
            println!(
                "wasi-sdk {WASI_SDK_VERSION} is already installed at {}",
                sdk.root.display()
            );
            return Ok(());
        }
    }
    let sdk = match archive {
        Some(a) => unpack_verified(a, pin.sha256, &dest)?,
        None => {
            let partial = cache.join(format!("{}.partial", asset_name(pin)));
            let url = asset_url(pin);
            // A complete archive left by an interrupted install must not be "resumed" (curl would fail).
            if !partial.is_file() || sha256_file(&partial)? != pin.sha256 {
                eprintln!("downloading {url}");
                // Resume a partial download; if that fails (e.g. the partial is stale or complete but
                // corrupt, which curl cannot resume), start over once from nothing.
                if download(&url, &partial).is_err() {
                    let _ = std::fs::remove_file(&partial);
                    download(&url, &partial)?;
                }
            }
            let unpacked = unpack_verified(&partial, pin.sha256, &dest);
            // Keep nothing: a bad download must not be resumed, and a good one is no longer needed.
            let _ = std::fs::remove_file(&partial);
            unpacked?
        }
    };
    println!(
        "installed wasi-sdk {WASI_SDK_VERSION} at {}",
        sdk.root.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pin_is_a_distinct_sha256_and_names_a_release_asset() {
        for (i, p) in PINS.iter().enumerate() {
            assert_eq!(p.sha256.len(), 64, "{}", p.platform);
            assert!(
                p.sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "{}",
                p.platform
            );
            assert!(PINS[..i]
                .iter()
                .all(|q| q.sha256 != p.sha256 && (q.os, q.arch) != (p.os, p.arch)));
        }
        let pin = pin_for("linux", "x86_64").unwrap();
        assert_eq!(
            asset_url(pin),
            "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-33/wasi-sdk-33.0-x86_64-linux.tar.gz"
        );
        assert!(pin_for("plan9", "x86_64").is_err());
    }

    #[test]
    fn unpack_checks_the_hash_and_strips_the_top_directory() {
        let tmp = std::env::temp_dir().join(format!("ibc-toolchain-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let top = tmp.join("src").join("wasi-sdk-0.0-test");
        std::fs::create_dir_all(top.join("bin")).unwrap();
        std::fs::create_dir_all(top.join("share").join("wasi-sysroot")).unwrap();
        std::fs::write(
            top.join("bin")
                .join(format!("clang++{}", std::env::consts::EXE_SUFFIX)),
            b"not clang",
        )
        .unwrap();
        std::fs::write(top.join("VERSION"), b"0.0\n").unwrap();
        let archive = tmp.join("sdk.tar.gz");
        let mut tar = Command::new(tar_program());
        tar.arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(tmp.join("src"))
            .arg("wasi-sdk-0.0-test");
        run(&mut tar, "tar").unwrap();

        let dest = tmp.join("wasi-sdk-test");
        let wrong = unpack_verified(&archive, &"0".repeat(64), &dest).unwrap_err();
        assert!(
            format!("{wrong:#}").contains("sha256 mismatch"),
            "{wrong:#}"
        );
        assert!(!dest.exists());

        let sha = sha256_file(&archive).unwrap();
        assert_eq!(
            sha,
            ibc_replay::sha256_hex(&std::fs::read(&archive).unwrap())
        );
        let sdk = unpack_verified(&archive, &sha, &dest).unwrap();
        assert_eq!(sdk.version().as_deref(), Some("0.0"));
        // A second install replaces the first.
        unpack_verified(&archive, &sha, &dest).unwrap();
        assert!(!dest.with_extension("unpack").exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
