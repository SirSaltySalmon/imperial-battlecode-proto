# Releasing and patching

The game will be patched often during a season. This page says what a patch is, what has to be true before
one ships, and how to switch the ladder to it and back.

## Patch identity

A match is a pure function of `(patch, map, bot A, bot B, seed)`. The **patch identity** is four values:

| Part | Where it comes from | Why it is in the identity |
|---|---|---|
| engine version | `version` in the root `Cargo.toml` (`ibc_engine::ENGINE_VERSION`) | the mechanics |
| ruleset hash | hash of the parsed `rules/*.toml` (`Ruleset::hash`) | every tunable number |
| Wasmtime version | the `=` pin in `crates/runner/Cargo.toml`, `WASMTIME_VERSION`, `Cargo.lock` | fuel accounting is version specific, so a bot can run out of fuel on a different tick |
| ABI version | `ibc_abi::ABI_VERSION` | what bots see and can do |

`ibc version --json` prints it for a binary; `cargo xtask patch-id` prints it for a checkout and fails if
the three places that state the Wasmtime version disagree. Every replay header and every match job carries
it, and a worker refuses a job whose patch is not its own (`docs/JOBS.md`).

The pinned Rust toolchain (`rust-toolchain.toml`) is not part of the identity, but it is treated like it:
it compiles the engine and the reference bots, and codegen and sort stability may change between versions.
A toolchain bump goes through the same checklist as a patch.

## What bumps when

| Change | Engine version | Ruleset hash | ABI version | Goldens |
|---|---|---|---|---|
| Balance only: numbers in `rules/*.toml` | **patch** (0.3.1 -> 0.3.2) | changes | same | re-record |
| Mechanics: engine code that changes any state hash | **minor** (0.3.x -> 0.4.0) | may change | same | re-record |
| ABI, additive (new query, new action, new field at the end) | minor | same unless rules changed | **stays**; document the addition in `docs/ABI.md` with the engine version that introduced it | re-record only if state hashes move |
| ABI, breaking | major, new season only | - | bumps | re-record |
| Wasmtime bump | **patch**, even with no other change | same | same | re-record the wasm goldens (fuel counts change) |
| Toolchain bump | patch if any golden moves, otherwise none | same | same | verify; re-record if they move |
| Engine refactor with identical state hashes, CLI, worker, coordinator, viewer, docs | none required | same | same | must verify unchanged |

Rules of thumb:

- The ruleset hash alone already separates two balance patches. The engine patch bump is there so that
  people have a name for the patch, so that the release tag is unique, and so that the viewer can pick its
  engine build by version.
- An additive ABI change must leave every existing bot module valid and behaving the same: old bots do not
  call the new imports. That is why the ABI version stays.
- One change per patch where possible (see `CLAUDE.md`): if a balance change and a mechanics change ship
  together, the league table cannot tell which one moved the numbers.

The release tag is `v<engine version>`; `release.yml` refuses a tag that does not match.

## Golden determinism

`fixtures/golden/` holds a small set of matches with their expected results. `cases.json` (map, seed, bot
pair) is edited by hand; everything else is generated.

| | What runs | What is compared |
|---|---|---|
| native (`native.json`, `native/*.hashes`) | engine + reference bots linked as Rust (`ibc-sim`), no Wasmtime | state hash of every tick, score, winner, tick count |
| wasm (`wasm.json`, `wasm/*.hashes`, `bots/*.wasm`) | `ibc run` with the pinned bot modules in `fixtures/golden/bots` | the same, plus the SHA-256 of `replay.ibcr`; the native engine must also re-verify the replay |
| wasm, fresh bots | `ibc run` with the reference bots built from source | state hash of every tick, score, winner, tick count |

```
cargo xtask golden verify            # both; --native or --wasm for one
cargo xtask golden record            # replay every case and rewrite fixtures/golden
```

`verify` names the case and the first tick whose state hash differs, for example
`state diverged at tick 520: expected 1bd6..., got b160... (identical through tick 519)`. Dump both sides
with `ibc replay dump` (or `ibc-sim run --frames`) and diff that tick to find the cause.

The bot modules are pinned because a compiled module embeds host paths, so two machines do not produce the
same bytes from the same source. With pinned modules every input is byte-identical everywhere, so the
replay SHA-256 must be too; that is what `determinism.yml` checks on four platforms. `record` keeps the
pinned modules as long as freshly built bots still behave identically, and rewrites them when they do not.

**A ruleset or mechanics change legitimately changes the goldens.** The workflow:

1. Make the change. `cargo xtask golden verify` now fails and shows where the matches diverge. Check that
   this is the divergence you expect (a balance change should not move tick 0; a refactor should move nothing).
2. `cargo xtask golden record`, and regenerate the viewer fixture named in `crates/engine-wasm/tests/fixture.rs`.
3. Commit `fixtures/golden` with the change. The PR diff of `native.json` and `wasm.json` shows the old
   and new ruleset hash, scores and final hashes; say in the PR that the goldens moved and why.
4. A reviewer checks that the goldens moved only in PRs that meant to move them. **Never re-record to make
   an unexplained failure go away:** an unexplained difference is a determinism bug.

## CI and what it costs

| Workflow | Runs | Does |
|---|---|---|
| `ci.yml` | PRs and pushes to `main` that touch Rust, rules, maps, bots, SDK or fixtures | one Linux job: `cargo xtask check` + native goldens; a second Linux job, only when something that can change a wasm match was touched: wasm goldens + `cargo xtask engine-wasm` |
| `viewer.yml` | changes under `viewer/` | builds the viewer |
| `determinism.yml` | by hand, on `v*` tags, weekly | `golden verify` + engine-wasm on Linux x64, Linux arm64, Windows x64, macOS arm64 |
| `release.yml` | `v*` tags; by hand as a dry run | six targets, archives, wheels, `SHA256SUMS`, draft GitHub Release |

Docs-only changes start nothing. PRs never run a matrix. Superseded runs are cancelled. Dependabot opens at
most one grouped PR per ecosystem per month and never touches Wasmtime.

`cargo xtask check` enforces `fmt --check` and `clippy -D warnings` on the packages listed in
`xtask/src/check.rs` and reports the backlog of the rest; once the tree has had its one formatting and
lint clean-up commit, add `--strict` to the CI step.

## Release checklist

1. **Scope.** One patch, one purpose. Decide the version from the table above and bump `version` in the
   root `Cargo.toml` (and the Wasmtime pin, constant and lockfile together, if that is the change).
2. **Measure** (balance and mechanics patches). League table before and after on the whole reference pool,
   both sides, several seeds: `cargo run -p ibc-sim --release -- league --maps maps --seeds 20`. Put both
   tables in the PR.
3. **Goldens re-recorded and reviewed.** `cargo xtask golden record`; the diff is in the PR and a second
   person has read it. `ci.yml` is green on the merge commit.
4. **Cross-platform determinism green.** Run `determinism.yml` by hand on the release commit and wait for
   all four platforms. Do not tag on a red or missing run.
5. **Shadow run.** Build the new `ibc` and re-run a sample of recent ranked inputs (same bots, maps and
   seeds; a few thousand matches, `bench` priority so the ladder is not delayed) on the new patch. Compare
   the outcome distributions with the live ones: attacker-side win rate, mean ticks, draw rate, per-map
   win rates, out-of-fuel turns and faults per match, and how many results flipped. A balance patch is
   expected to move some of these, by about the amount the PR predicted; faults, out-of-fuel turns or
   unfinished matches appearing where there were none is a stop. Results of shadow jobs are never rated.
6. **Tag** `v<engine version>` on `main`. `release.yml` builds the six targets and opens a draft release
   whose notes contain the patch identity. Check that the identity in the notes is the one from step 3,
   check `SHA256SUMS`, then publish the draft.
7. **Keep the build forever.** Release assets are never deleted or replaced, and the worker image or
   binary of every patch that ever rated a match is kept. Old replays can only be re-verified, and old
   results can only be spot-checked, by the patch that produced them. A fix to an old patch is a new patch.
8. **Viewer.** The viewer ships one engine wasm per released engine version
   (`cargo build -p ibc-engine-wasm --release --target wasm32-unknown-unknown` at the tag) and picks the
   one named in the replay header. Add the new build; never remove an old one. A replay loaded with the
   wrong engine fails at load (every state hash is checked) instead of drifting.
   Release archives and wheels carry the built viewer too (`release.yml` builds it once in its `viewer`
   job), so `ibc view` works offline right after install.
9. **Wheel** (optional, manual). The wheels from the draft release can be uploaded to PyPI with
   `twine upload`; CI does not publish.
10. **Switch the ladder** (next section), then announce the patch with its identity and the balance table.
11. **Tell participants' `ibc`.** Copy the release's `distribution.json` (every archive with its URL and
    SHA-256, written by `cargo xtask dist distribution`) to the coordinator's `IBC_DISTRIBUTION` path.
    From then on `ibc` warns participants on an older version within a day and `ibc update` prints their
    upgrade command (`docs/DEPLOY.md`, "Publishing a release to participants").

## What is in a release archive

`ibc-<version>-<target>.tar.gz` (`.zip` on Windows) unpacks to one directory with `ibc`, `ibc-worker` and
`ibc-coordinator`, `viewer/` (the built replay viewer with its engine wasm), `maps/` (the map pool),
`docs/` (participants' guide when present, rules, ABI), `README.md`, `PATCH.json`, and on Linux the
systemd units `ibc-worker.service` and `ibc-coordinator.service`. `cargo xtask dist --target <triple>`
builds one locally (`--viewer DIR` or `--no-viewer` if `viewer/dist` is not built) and rewrites
`dist/distribution.json` for what is in `dist/`.

## Switching the coordinator's current patch

The coordinator stamps every new match job with its **current patch** `{engine, ruleset_hash, wasmtime}`;
workers only run jobs whose patch equals their own `ibc version --json` and release others, so a mixed
fleet is safe, only slow. The switch is explicit, and what it does to running series, ratings and
submissions is fixed (docs/JOBS.md, "Patch boundary"):

1. Roll out workers with the new `ibc` next to the old ones (or register the new runner as `runner_blobs`).
   Nothing changes yet because no job asks for the new patch.
2. Set the coordinator's current patch to the new identity, exactly as printed by `ibc version --json` of
   the released binary: `POST /v1/admin/patch` with `{"engine", "ruleset_hash", "wasmtime"}`, plus
   `"balance": true` for a balance patch and `"imports": {"ibc": [...], "wasi": [...]}` if the patch changes
   what bots may import and the coordinator build does not know it yet. There is no need to pause ranked
   scheduling or drain the queue first:
   - series that have started finish on the old patch, and so does every tournament series; series that
     have not started are re-planned on the new patch. Nothing is voided;
   - old-patch jobs still queued after `IBC_PATCH_DRAIN_SECS` (default 1 h) are dead-lettered, except
     tournament jobs, so keep at least one old worker (or the old `runner_blobs`) for that long, and until
     every tournament on the old patch is over;
   - a balance patch widens every team's rating uncertainty (`IBC_BALANCE_SIGMA_FACTOR`, default 1.5), so
     ratings move faster while they settle on the new game. Other patches leave ratings alone;
   - every live submission is checked against the new patch. The response lists the ones that fail. Their
     teams keep playing for `IBC_INVALID_GRACE_SECS` (default 24 h, as Cambridge gave), then are left out of
     ranked pairing until they upload a valid bot. Tell those teams: `GET /v1/team` shows them
     `live_status: "grace"` and the reason.
   From here on every new job carries the new patch, and the idempotency key (which includes the patch)
   makes old results unreachable for new submissions.
3. Watch the first hour: `GET /v1/patch` (`draining_jobs`, `teams_in_grace`, `teams_invalid`), the
   dead-letter list, `patch mismatch` releases (a worker left on the old build), spot-verification
   mismatches (should be zero; a mismatch between two workers on the same patch is a determinism bug and
   stops the ladder).
4. Retire old workers when `draining_jobs` is 0. Keep the binary (checklist step 7).

A full rating reset is a competition decision, belongs in the patch announcement, and is not in the tooling.

## Rollback

A patch is rolled back by switching the current patch back; nothing is rebuilt.

1. Pause ranked scheduling.
2. Set the coordinator's current patch back to the previous identity (without `balance`). The old workers
   or the old binary are still there because builds are kept. The patch boundary applies as for any switch:
   started series finish on the bad patch, the rest are re-planned on the restored one.
3. Decide what happens to matches rated on the bad patch: they are identified exactly by the patch in
   their summaries, so they can be excluded from ratings and, if wanted, re-run on the restored patch
   (same bots, maps and seeds).
4. Resume, and write down what the shadow run missed so the next one catches it.

Do not delete the bad release or reuse its version number: replays of it exist. Fix forward with the next
patch version.

## What only a real CI run proves

Local runs prove that the goldens are stable on one machine. Identical results on other operating systems
and CPU architectures, the six-target release build, the zig cross build and the wheels are proven only by
`determinism.yml` and `release.yml` (the latter can be started by hand as a dry run without tagging).
