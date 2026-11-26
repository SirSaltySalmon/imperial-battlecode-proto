//! Repository tasks, run as `cargo xtask <task>` (alias in `.cargo/config.toml`).

mod check;
mod dist;
mod dist_contents;
mod distribution;
mod engine_wasm;
mod golden;
mod patch_id;
mod util;

use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask <task>
  check [fmt] [clippy] [test] [--exclude <pkg>]... [--strict]       what CI runs on every PR
  golden record|verify [--native] [--wasm] [--no-fresh]           golden determinism (fixtures/golden)
  engine-wasm [--require-node] [replay.ibcr ...]                  build the browser engine, compare it with native
  dist --target <triple> [--builder cargo|zigbuild|cross]         release archive + distribution.json in dist/
       [--viewer DIR | --no-viewer] [--base-url URL]
  dist sums [dir]                                                 write SHA256SUMS
  dist distribution [dir] [--base-url URL]                        rewrite distribution.json from the archives
  dist pip-data [--viewer DIR | --no-viewer]                      stage viewer, maps, docs for the pip wheel
  patch-id [--json | --markdown] [--bin <ibc>]                    engine, ruleset hash, wasmtime and ABI versions";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    let result = match args.first().map(String::as_str) {
        Some("check") => check::main(rest),
        Some("golden") => golden::main(rest),
        Some("engine-wasm") => engine_wasm::main(rest),
        Some("dist") => dist::main(rest),
        Some("patch-id") => patch_id::main(rest),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
