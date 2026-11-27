# TODO

What is left to do, as of 2026-11-26, after the foundation, the first wave of eleven units, the job-system
integration and the twenty-unit proof-of-concept batch (PRs 16 to 35, merged through the `integration` branch)
landed. Each item says what is wrong or missing, where it lives, and what "done" looks
like. Items inside a section are in the order I would do them.

Where an item came from a measurement, the number is given so it can be re-checked. `ibc-bench`
(`docs/hillclimb.md`) is the tool for anything about balance; `cargo xtask golden verify` is the check that a
change did or did not alter simulation.

## 1. The game is not yet a good game

Measured with `ibc-bench balance` on the foundation rules and two maps (native, 5 seeds, 390 rounds). Re-run
it on the current ten-map pool before acting; the numbers below predate the new weapons, abilities and maps.

- [x] **The objective never decides a round.** Was 0 of 390 rounds on the foundation rules. Re-measured on
  2026-11-25 with the eight-bot pool on ten maps (20,609 rounds): 11.6% end by detonation, 28.0% by defuse,
  59.4% by elimination, 1.0% on the clock, and the charge is planted in 69.3% of rounds. It was the bots:
  the old two-bot pool on the same rules and maps gives 19.8% / 10.8% / 68.0%. Table in `bots/README.md`.
- [ ] **Mirror matches are lopsided.** Re-measured 2026-11-25: attackers win 26.5% of `rusher` mirror
  rounds, 30.7% of `forcer`, 85.4% of `holder`, and 67 to 74% of the other squad bots' mirrors. Matches decided
  by the tie-break fell from 29.2% to 7.9% with the larger pool. Done: attacker round win rate per pairing,
  not just overall, sits in a band we choose (say 40 to 60%) for bots of comparable strength.
- [x] **Reference bots are too few and too alike.** Eight playing bots now (`bots/README.md`): `rusher`,
  `holder`, `utility` and five styles of one shared implementation, `bots/squad` (`sapper`, `retaker`,
  `lurker`, `saver`, `forcer`). All eight are in the sim registry. The squad bots draft unique kits, use
  abilities for the objective, buy different weapons and vary with the seed (1,800 of 1,920 games distinct).
  Left over: `rusher` and `utility` still ignore the seed, and `holder` is too weak a defender to be a
  useful opponent (it loses 84 to 92% of defending rounds to any bot that attacks as a group).
- [ ] **`forcer` beats the whole pool** (78.8% of matches) with the cheapest loadout and the most forward
  defence. Find out which of the two it is with `ibc-bench why` and an A/B of `forcer` against a copy with
  `Buy::Best`, before touching SMG or shotgun prices.
- [ ] **The batch moved the league a long way, and nothing is tuned yet.** Native league, 14 maps x 3 seeds,
  3,024 games, 672 per bot (about +/-3.7 points), win % before the batch -> after: rusher 65.2 -> 53.1,
  holder 37.9 -> 28.7, utility 45.5 -> 33.3, sapper 53.4 -> 52.2, retaker 49.6 -> 73.7, lurker 52.1 -> 53.4,
  saver 63.8 -> 76.0, forcer 82.4 -> 79.5. Most of it is the firing model (unit 3, PR 26): bots that shot on the
  move lost, patient ones gained. Retaker's first jump came with draws and the blast (unit 1, PR 33); read
  replays before acting on it. Per-stage tables are in the integration PR. Tune one change at a time.
- [ ] **Pistol and rifle after the firing model.** With no movement spread and no kick, the pistol kills at
  20 tiles and the rifle is strong at every range (unit 3's note). Re-measure kills by weapon, then tune.
- [ ] **Refused actions during ability startup.** Since thrown abilities gained a startup (unit 5), bots
  built on `utility` see 0.8 to 1.4% refused actions: they try to shoot while casting. Teach the SDK and bots
  to wait out `S_CASTING`.
- [ ] **New mechanics are played but not tuned.** Kills by weapon with the eight-bot pool: rifle 24.8%,
  marksman 23.2%, pistol 18.7%, SMG 9.7%, abilities 9.2%, shotgun 8.0%, LMG 6.5%. The marksman rifle is
  carried by about one unit in five and takes almost a quarter of the kills. Tune one thing at a time
  (`CLAUDE.md`, "Changing the game").
- [ ] **Three maps are lopsided,** but not the three listed before. With the eight-bot pool attackers win
  72% of rounds on `crossfire`, 76% on `tunnels` (the two original maps, defenders spawn next to the sites)
  and 66% on `longwatch`; `postern` is the low one at 38%. `quarry`, `trident`, `agora`, `culvert`,
  `spillway` and `warrens` sit between 45 and 54%. Per-map table: `docs/maps.md`.
- [ ] **Native and wasm disagree when a bot runs out of fuel.** The five squad bots no longer do: 40 wasm
  matches between them had no out-of-fuel turn (largest turn 297,188 of the 400,000 a buy tick allows),
  because they spread their path searches over the buy ticks and keep the results. `holder` still plans a
  whole round in its first buy tick and overruns it (7 turns in one match on `quarry`). Give it the same
  treatment or retire it, and make the bench default to the wasm backend for anything that is reported.
- [ ] **Pathfinding cost on large maps.** A full distance field on `crossfire` (544 tiles) is roughly one
  100k-fuel tick by estimate, never measured. On 56x40 maps `path_len`/`approach_point` in `sdk/rust/src/nav.rs`
  may exceed even the buy-phase budget. Measure fuel per call with the wasm runner and either budget the search
  or offer a host-side path helper in a later ABI revision.
- [x] **Friendly fire for bullets.** Decided (vision): abilities hit your own team, bullets never do.
- [ ] **`SiteInfo` is only a bounding box.** Its centre need not be a site tile on L-shaped or hollow sites, so
  `A_PLANT` there would fail. `ibc-maptool lint` rejects such maps today; the better fix is a guaranteed
  plantable point per site (additive ABI field).
- [x] **Bots cannot see their weapon cooldown or second weapon.** Done: `UnitInfo.other_weapon` and
  `fire_cooldown` (unit 4). `UnitInfo` shows only the weapon in hand and
  has no weapon cooldown, so bots get `ERR_NOT_READY` unless they track it. Additive ABI fields.
- [x] **`vx`/`vy` report the commanded velocity**, even when a wall blocked the move. Report what happened.
- [ ] **`EVENT_DAMAGE` carries no weapon id**, so damage-by-weapon needs re-simulation. Additive.

## 2. Engine and runner correctness

- [ ] **C++ bots that use exceptions are rejected.** `ibc_runner::check_wasm` fails with "exception refs not
  supported". Enable the exception-handling feature in the runner's Wasmtime config
  (`crates/runner/src/lib.rs`), confirm fuel accounting still holds across a throw/catch (Wasmtime 50 changed
  it), add a test bot that throws, and re-record goldens. Until then `sdk/cpp/README.md` says to stay on the
  default `-fno-exceptions`.
- [ ] **The runner scales badly inside one process.** Per-match cost was about 26 ms at `--jobs 1` and 88 ms
  at `--jobs 8` (8.6 to 8.8 matches per core-second instead of 32 to 35). The worker works around it with one
  runner process per slot. Find the contention in `crates/cli/src/batch.rs` / `crates/runner` (likely the
  shared module cache lock or allocator) before sizing a fleet.
- [ ] **The `unsafe` in the fuel logic.** `crates/runner/src/vm.rs` reaches the store through a raw pointer to
  re-arm a suspended bot's fuel each turn. It is tested, but it relies on Wasmtime 51 internals. Re-check on
  every Wasmtime bump, and ask upstream whether a safe API exists.
- [x] **Bot debug drawings never reach the viewer.** Done in replay format 2 (docs/REPLAY.md). `frame.debug` is empty on re-simulation: `debug_draw`
  calls are not recorded as replay inputs and `World::step` clears them. Record them in the replay as
  non-simulation events (they must not affect the state hash).
- [x] **Core rules are not in the replay header.** Done in replay format 2 (docs/REPLAY.md). The viewer hardcodes vision range, cone angle, unit radius
  and more in `viewer/src/vision.ts`; a balance patch will make drawn cones disagree with what units saw. Put
  the ruleset tables (or the whole TOML) in the header and have the viewer read them.
- [x] **Per-round and per-kill statistics in `summary_json`**, a per-turn fuel distribution in the runner
  summary, and a seed-independent game digest. Done (docs/REPLAY.md, "Match summary"): `game.rounds[].planted`,
  `game.kills`, `game.digest` and `teams[i].fuel.turn_p50/p90/p99`. `ibc-bench` takes plants, kills by weapon
  and exact repeats from the summary; it still re-simulates wasm replays for first contact, kill distance,
  movement, economy, loadouts and shots by weapon. Was: `ibc-bench` re-simulated every replay to get these.
- [ ] **Stack-overflow depth is probably architecture-dependent.** A bot that recurses until it traps may
  fault at a different tick on another CPU. Test on arm64 (section 4) and, if so, bound it deterministically.
- [ ] **`memory.copy`/`memory.fill` cost fuel per byte.** Documented for players; consider setting
  `Config::operator_cost` so a 100 KB `memcpy` does not consume a whole tick.
- [ ] **`lead` and `dist` in the Rust SDK overflow** on extreme inputs (a bullet speed near 1): wraps in
  release, panics in debug. Clamp or use wider arithmetic in `sdk/rust/src/geom.rs` and mirror it in
  `sdk/cpp/ibc.h` (the C++ port matches release behaviour on purpose).
- [ ] **`ibc build --debug`** and bots that print to stdout have never been exercised.

## 3. Job system and platform

- [ ] **Deployment files have never been run.** `deploy/Dockerfile`, `docker-compose.yml`,
  `ibc-worker.service` and `cloud-init.yaml` are unexecuted. The compose file's coordinator service was written
  before the coordinator existed, and `cloud-init.yaml` points at a release download that does not exist yet.
  Done: `docker compose up` brings up a coordinator and two workers that play a ranked batch; a fresh Linux VM
  booted with the cloud-init file registers and plays.
- [ ] **Drain on SIGTERM has only been tested through the drain file.** Windows kills the process outright.
  Test on Linux before relying on it for spot instances.
- [ ] **`--allow-runner-fetch` and side-by-side runner directories have no test.** This is the path that lets
  a fleet change patch without redeploying; it downloads and runs native code, so it needs a careful test.
- [x] **No web platform.** Done: `web/` (unit 13) with email + password accounts, sessions and device-code
  CLI login (unit 8), served by the coordinator at `/` with the viewer at `/viewer/`. Was: There is an HTTP API and nothing for a human: no sign-in, team pages, submission
  upload, ladder, match list or embedded viewer. This is the largest missing piece for an event. The
  coordinator authenticates teams with a bearer token shown once at creation; real sign-in (Cambridge used
  Discord and email) has to be designed.
- [x] **How the ladder shows a new team.** Done: placement. A new live submission plays 3 ranked series back
  to back; it shows its rating from 1500 (Weng-Lin now displays `mu`) and "Provisional" instead of a tier
  until they finish. Before that: "Provisional" with no number until 5 rated series (unit 9). Was: The API exposes `rating` and `provisional`; the frontend must
  decide whether a provisional team shows a number or the word. With Weng-Lin a new team's number starts near
  763 and climbs.
- [x] **No tournament runner.** Done: `docs/api/tournaments.md` (lock snapshot, seeded single/double
  elimination, every pairing plays every map, maps hidden until revealed, ranked paused at lock).
- [x] **A patch change voids running series.** Done (unit 12): started series finish on their patch, queued
  ones move, balance patches widen ratings. Was: Setting a new patch dead-letters queued and leased jobs of the
  old one at once. Decide the behaviour at a patch boundary (finish in-flight series on the old patch, or void
  them) and whether ratings widen (`on_balance_patch` exists in `ibc-rating` and is not called).
- [x] **Invalid submissions after an ABI or engine change.** Done (unit 12): 24 h grace, then out of
  matchmaking. Was: There is no server-side rebuild because bots are
  uploaded as wasm. Decide the rule for bots that no longer validate (Cambridge gave a 24 h grace period, then
  automatic losses).
- [x] **Language fuel multipliers are self-declared.** Done (unit 12): the server's table decides; `ibc build`
  embeds an `ibc.language` section. Was: Harmless while only Rust and C++ exist (both 1x). Before
  Python arrives the server must decide the multiplier itself, not trust the upload.
- [ ] **SQLite and local disk only.** Fine for one box. `crates/coordinator` keeps SQL in `src/db/` and blobs
  behind a trait for a later Postgres and S3 backend. Not needed until a second coordinator or off-box storage
  is.
- [x] **Backups and retention.** Done: `ibc-coordinator backup`, `prune-replays`, `deploy/backup.sh` (unit 16),
  per-team submission retention (unit 11). Was: Document and automate copying the SQLite file and blobs. Add a replay
  retention policy: replays can be regenerated from (patch, map, bots, seed), so unranked and bench replays
  need not be kept forever.
- [x] **Abuse limits.** Done (unit 11): per-token and per-IP request limits, uploads per hour, module size,
  429 with `Retry-After`, suspend/ban for teams and users. Was: Upload rate and size per team, total submissions kept per team, API rate limits.
- [ ] **Cost prior in the budget governor.** `series_per_batch` falls back to 10 core-seconds per game when it
  has no measurement; measured cost is about 0.03. Unit 9 made it schedule at least one series per round until
  games are timed, and devstack uses a 30 s interval; the prior itself is still 10.
- [ ] **Plagiarism and duplicate detection.** Identical wasm across teams is flagged (unit 11); near duplicates
  are not.
- [ ] **Accounts:** emails are not verified, so register the `IBC_ADMIN_EMAILS` accounts before the site is
  public; login throttling is per email only (put per-IP limits in the reverse proxy).
- [ ] **Any worker token can read any full replay blob** by hash. Fine while workers are trusted.
- [ ] **Double elimination has no bracket reset** in the grand final (documented in `docs/api/tournaments.md`).
- [ ] **Docker images have never been built** (no daemon on the dev machine); `docker compose config` accepts
  the file.

## 4. Never executed

- [ ] **Cross-platform determinism.** `.github/workflows/determinism.yml` (Linux arm64, Windows, macOS arm64)
  has never run. Results are proven identical on Windows x64 and Linux x64 only. Trigger it by hand from the
  Actions tab now that the file is on `main`; this is the proof behind "runs the same on any hardware".
- [ ] **Release pipeline.** `.github/workflows/release.yml` (six targets, zig cross build, pip wheel, draft
  release) has never run. Use its `workflow_dispatch` dry run before the first tag.
- [ ] **`ibc toolchain install` download.** The sha256 pins are the digests GitHub publishes, but no archive
  was ever fetched.
- [ ] **Workflow lint.** The YAML was parsed, not linted. Run `actionlint`.
- [ ] **The three shell wrappers in `tools/`.** (`tools/devstack.sh` has run green.)
- [ ] **CI did not run on PR 15.** Check why (path filters or a trigger) so job-system changes are covered.
- [x] **A full workspace test run.** 488 passed on the integration branch.

## 5. Tests and code health

- [ ] **Flaky test:** `view::tests::serves_the_replay_over_http` in `crates/cli/src/view.rs` failed once under
  a full parallel run and passed alone.
- [ ] **CI needs wasi-sdk** for the C++ geometry and starter-build tests; without it they skip silently.
- [ ] **Formatting and lints are enforced on `xtask` only.** 33 files are not `rustfmt`-clean and there are 14
  clippy warnings in `ibc-abi`, `ibc-engine`, `ibc-sim` (`ibc-cli` and `ibc-runner` unsurveyed). Do one
  clean-up commit, add a `rustfmt.toml` (`max_width = 120`, `use_small_heuristics = "Max"` gave the smallest
  diff), then switch CI to `cargo xtask check --strict`.
- [ ] **`ibc_runner::run_driven` is about 190 lines** (it grew out of `run_match` for `ibc play`).
- [ ] **`crates/engine-wasm/src/view.rs` serialises rules fields by hand,** so a new rules field must be added
  there too or the viewer's buy menu will not show it.
- [ ] **Generated rules page has empty meanings** for `EconRules` and many core fields: they have no doc
  comments in the engine.
- [ ] **C++ templates and headers from this batch were never compiled** (wasi-sdk not installed).
- [ ] **Functions over the 80-line guideline:** `drawEffects` in `viewer/src/draw-world.ts`, `ab::analyse`,
  the Markdown renderers and `dump::why` in `crates/bench`, two in `crates/coordinator`.
- [ ] **`ibc-bench` accepts flags meant for another subcommand** and ignores them.
- [ ] **`ibc-bench` reads `World` fields directly,** so a rename in `world.rs` breaks its build. Give it a
  stable accessor.
- [ ] **The golden bots are committed binaries** (`fixtures/golden/bots/*.wasm`) because a compiled bot embeds
  host paths. Try `--remap-path-prefix` so the same source builds to the same bytes everywhere, then drop them.
- [ ] **`fixtures/sample.ibcr` is refreshed by hand** after a rules change; `cargo xtask golden record` does
  not do it. Fold it in.
- [ ] **Dependabot PRs 13 and 14** are open and unreviewed. Wasmtime is excluded from them on purpose.
- [ ] **The viewer has only been checked on desktop Chromium.** Touch pinch-zoom is untested; ability effects
  and weapon drops are drawn but were never seen with real data.

## 6. Second wave (planned, not started)

- [ ] **Python SDK.** Stock CPython 3.14 or 3.15 on WASI, a Wizer snapshot after importing the bot's modules,
  and the host API either through a statically linked extension module (needs a custom CPython build) or over
  a file descriptor the WASI shim implements (no rebuild). Cambridge gave Python 25x the fuel; that multiplier
  is the main running-cost lever, so choose it deliberately.
- [x] **Map editor** in the browser: `ibc view --editor` (unit 20).
- [ ] **Bot parameter tuner** (SPSA, as `tools/evotune` did for the Cambridge bot) on top of `ibc-bench`.
- [x] **Handbook generator:** rules pages from `rules/*.toml`, the ABI reference from `crates/abi`, and a
  single Markdown bundle for AI agents, as Cambridge's "Download for LLM". `cargo xtask docs`, output in
  `docs/generated/`, checked in CI. Left over: serve `llm.md` from the platform as a download.
- [x] **CLI update check and `ibc.toml` version pinning:** `ibc update` prints the upgrade command, other
  commands warn once a day when outdated (unit 16). It never downloads by itself.
- [ ] **More bot languages** once the ABI has settled (Go, Zig, AssemblyScript are cheap on `wasm32-wasip1`).
- [ ] **Shadow runs before a patch goes live:** replay yesterday's ranked inputs on the new engine and compare
  outcome distributions (`docs/release.md` describes it; nothing implements it).

## 7. Decisions needed

- Eligibility categories for the event (sign-in is email + password, decided).
- Ladder cadence. Ranked series are now 3 games (sides alternate per game) every 600 s by default.
- Team shared memory size (64 bytes now). (Sightings and sounds are now shared automatically.)
