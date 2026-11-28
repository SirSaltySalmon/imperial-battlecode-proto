# Several agents on one machine

How to split work across parallel workers (agents in worktrees) without the machine stalling. `CLAUDE.md`
carries the short form; this page has the reasons and the numbers.

## What went wrong

Measured from the transcripts of 25 to 27 November 2026 (199 transcripts, 9,494 shell commands) on the
development machine, which has 20 logical cores and 16 GB of RAM:

| Observation | Number |
|---|---|
| A plain `grep` or `cat`, machine quiet | about 1 s median |
| The same, during a fan-out | 12 to 30 s median |
| `cargo build --release -p ibc-cli -p ibc-sim -p ibc-bench`, one worker, cold target directory | 29 to 47 min |
| `tools/bench.sh balance --seeds 5`, one worker (78 to 88 s for `--seeds 3` on a quiet machine) | 17 to 56 min |
| `cargo xtask docs`, one worker | 21 to 47 min |
| `cargo xtask golden`, one worker | 6.6 min median, 21 min worst |
| Foreground time spent in `until ...; do sleep` loops waiting for a build | about 3 h |
| A spot check during a fan-out | 33 `rustc` and 16 `cargo` processes, 0.7 GB of RAM free |

Three causes, in order of cost:

1. **Memory.** Each cargo starts one `rustc` per core, so five workers ask for a hundred. Release builds of
   Wasmtime and Cranelift take more than a gigabyte per `rustc`; on 16 GB the machine pages, and then every
   process is slow, including the ones that are not compiling.
2. **Every worker compiled everything.** Workers have separate target directories (a shared one makes
   worktrees link each other's crates), so each one rebuilt all 268 packages, and the balance workers did it
   twice: once for their branch and once for a checkout of the base commit.
3. **Workers repeated each other.** Five workers measured the same "before" table on the same base commit.
   Workers whose change was in the engine built the release `ibc` and ran the wasm goldens.

## The verification ladder

Stop at the lowest rung that proves the change.

| Rung | Command | Needs Wasmtime | Use it for |
|---|---|---|---|
| 1 | `cargo check -p <crate>` | only for `runner` and `cli` | every edit while iterating |
| 2 | `cargo test -p <crate>` | as above | the crate you changed |
| 3 | `cargo run -p ibc-sim --release -- league ...`, `ibc-bench ... --backend native` | no | match behaviour: mechanics, rules, bots |
| 4 | `cargo xtask golden verify --native` | no | a mechanics change that must not move the goldens |
| 5 | `ibc run` + `ibc replay verify`, `cargo xtask golden verify --wasm`, metered `ibc-bench` | yes, release | changes to the runner, the CLI, the ABI, the SDKs, fuel |
| 6 | `cargo xtask check`, `cargo test --workspace`, `cargo xtask docs`, full `golden verify`, the byte-identical replay check | yes | the integrated branch, once |

Rungs 1 to 4 for an engine change compile the engine and a handful of small crates. Rung 5 compiles Wasmtime
in release mode. Rung 6 belongs to whoever merges the workers' branches, not to the workers.

A balance change still reports metered numbers (`docs/hillclimb.md`): iterate on the native backend, then run
the metered `balance` once on the final version.

## Rules for the orchestrator

- **Size the fan-out to memory.** Count `rustc` processes across all workers, not workers. On the 16 GB
  machine keep the total near 8: for example four building workers with `CARGO_BUILD_JOBS=2`, or two with 4.
  Workers that only read, or only run rungs 1 to 3 on small crates, cost little and do not count the same way.
- **Build shared things once.** Before starting workers, build the base commit's release binaries and run the
  "before" bench once. Give every worker the paths: `IBC_BIN=<base>/release/ibc.exe` for baseline runs and the
  `before` output directory for `tools/balance.sh compare`. Matches are deterministic, so a second copy of
  either is wasted compute.
- **Give each worker** its own `CARGO_TARGET_DIR`, `RUSTC_WRAPPER=sccache` and its `CARGO_BUILD_JOBS`, and say
  which rung its change needs.
- **Run rung 6 yourself**, once, after merging.
- **Remove a worker's target directory** when its branch is merged. They are about 4 GB each.

## Rules for a worker

- Use the rung your brief names. If you think the change needs a higher one, say so in your report instead of
  running it.
- Start anything longer than about two minutes in the background and carry on; the harness notifies you when
  it ends. Do not poll it with a sleep loop.
- Do not `cargo clean`, and do not build with `--release` unless the rung needs it.
- Keep scratch files in `out/` inside your own worktree.

## sccache

[sccache](https://github.com/mozilla/sccache) caches compiler output by input, so a crate from the registry
that one worker compiled is a cache hit for every other target directory. Set it per command rather than in
`.cargo/config.toml`, so that CI and the Docker build do not need it:

```
export RUSTC_WRAPPER=sccache CARGO_TARGET_DIR=<the worker's own> CARGO_BUILD_JOBS=<its share>
```

Its limits: workspace crates in different worktrees do not share hits (their absolute paths differ, and they
build incrementally), and crates that link (binaries, proc macros) are never cached. It removes the repeated
compile of the dependencies, which is most of a cold build, and nothing else.

What has been measured: building the same three packages into a second, empty target directory, 115 of the
175 cacheable compiles were cache hits. Both runs were on a machine already saturated by other workers and
both hit a `rustc` failure there, so there is no clean timing yet and it is not established whether sccache
or the lack of memory caused the failures. Until a run on a quiet machine settles that, a worker whose build
fails under sccache should retry once with `RUSTC_WRAPPER` unset and report it.

`sccache --show-stats` prints hits and misses; the cache location and size are set in sccache's own `config`
file (no extension) under `%APPDATA%\Mozilla\sccache\config` on Windows.
