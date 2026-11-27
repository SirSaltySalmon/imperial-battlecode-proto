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
  The native sim (`crates/sim`) has no memory cap at all: a bot over `memory_bytes` only fails on wasm.
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

- [ ] **Fuel does not bound a match's CPU or memory: some host calls cost far more than they charge.**
  Found 2026-11-27 with the kernels in `bots/burner/examples`, full idle-length games, native `ibc`:
  - Fixed: `mem_write` stored `Input::MemWrite { data }` in the replay before any check, so refused 1 MiB
    writes grew the runner by about 23 MB a tick. Now a refused `mem_write` or `act` leaves no input,
    accepted writes are kept once per team and scope a tick (the bytes they left), and `act` accepts at most
    `MAX_ACTS` (64) per unit and tick (docs/REPLAY.md, "Inputs"). Mirrors on crossfire: `mem_write` took
    705 MB after 25 ticks before, 26 MB for the whole match after; looping accepted 64-byte writes took
    760 MB after 100 ticks before, 16 MB and a 58 KB replay for the whole match after; looping moves took
    a 7.6 MB replay after 100 ticks before, 2.8 MB for the whole match after.
  - Fixed: `mem_read` allocated a zeroed buffer of the requested length before the engine refused it. The
    engine now checks first and lends its bytes, which the runner copies straight into guest memory, and
    `mem_write` passes the guest's bytes in place. The first 60 ticks of the `mem_read` mirror took 166
    core-s before and 2.7 after (on a loaded machine, where the plain burner took 3 core-s a match). A
    whole match is 68 core-s, as for any other host call made back to back at 20 fuel (`mem_spam`: 56).
  - Done (`fix/host-call-fees`): `debug_draw` cost 0 fuel and about 280 ns a call: 74 core-s per game, a 14 MB
    replay, 568 MB peak. Now 140 fuel and 2,048 drawings per team and tick: 33 core-s (84 before, same
    machine and hour). The rest is storing 4,096 drawings a tick at about 3 µs each; a lower cap is the next
    lever. WASI `fd_write` (64 iovecs) and `random_get` (4 KiB) were worse, 53 and 242 ns per fuel; now 5 per
    iovec and 2 per 8 bytes, at the table-read rate. `log` and `fuel_remaining` are no dearer per fuel than a
    table read and stay free. Fees need the engine patch bump (`docs/release.md`).
  - `count`/`get` and `los` spend about 1.9 ns per fuel, about 5 core-s per game. Partly fixed: reading a
    perceived table row by row redid the visibility pass per row (n passes, now one a tick: 11 times
    faster for 14 of 64 drops), `get` allocated a row buffer per call, and the `los` walk divided at
    every step. The `reads` kernel costs 12 to 15% less, `los` about 3% (noise). What is left is mostly
    the fixed cost of any host call (the charge, the game lock, Wasmtime's async call path).
  Per unit of fuel, arithmetic costs 0.18 ns, `count`/`get` and `los` about 11 times that, cache-missing
  loads about 43 times, `mem_read` about 1,600 times, and `debug_draw` is free. Fix these before trusting
  the cost prior (section 3).
- [x] **C++ bots that use exceptions are rejected.** Done: the runner enables Wasmtime's exception handling
  in the standard (exnref) encoding, with exception objects in a null-collector GC heap (no collection can
  suspend a bot), and the coordinator's upload check accepts the same proposal; both refuse the legacy
  encoding. `crates/runner/tests/exceptions.rs` shows fuel is exact across throw and catch, also when a turn
  runs out inside a catch handler, and that a throw/catch costs 1 fuel per instruction. Existing modules
  were not affected: the goldens verify unchanged. Left over: never compiled with wasi-sdk. Build a C++ bot
  with `exceptions = true`, check it passes `ibc build`, and measure its module size and fuel per `throw`
  for `sdk/cpp/README.md`.
- [x] **The runner scales badly inside one process.** Measured again on 2026-11-27 (i7-12700H laptop: 6
  two-thread performance cores and 8 efficiency cores, Windows 11; 200 rusher/holder matches on `crossfire`
  and `quarry` kept N in flight through `run-batch --jsonl`; high priority, as the box was shared; 3
  repeats, ranges in brackets). It does not triple. One process with `--jobs N` and N processes with
  `--jobs 1` cost the same per match up to N = 8 (66 [64, 70] against 66 [61, 74] ms); at 16 the one
  process is 11% slower (89 [88, 90] against 80 [79, 83] ms). The rise from 54 ms at N = 1 is the CPU
  (efficiency cores, shared cores, one power budget): separate processes rise the same. The old 26 and 88 ms
  were taken next to ten other builds. What one process adds: creating and dropping a unit's store maps and
  unmaps its memory, table and fiber stack, which serialises on the process's address-space lock (a
  benchmark doing only that: about 10,000 stores/s on Windows at any thread count; Linux under WSL2: 20,000
  on one thread, 10,000 on 16). Wasmtime's pooling allocator removes that on Linux (98,000 stores/s on 16
  threads), not on Windows, where it pools no fiber stacks and decommits memory on reuse. But a match makes
  only 10 stores, under 2 ms of 55: on whole matches pooling moved nothing outside the noise (Windows 73.8
  against 75.1 ms at 16 threads, Linux 49.6 against 48.7), so the runner does not use it. The module cache
  lock (two lookups per match), the per-match `Arc<Mutex<Match>>`, the heap (glibc tunables that keep large
  blocks changed nothing) and logging never showed. Done instead, the largest avoidable cost found: the
  replay was deflated twice per match, for `replay_sha256` and again to write `replay.ibcr`.
  `MatchOutput::replay_bytes` now carries the first encoding; a match costs about 0.8 of before at every
  `--jobs` (paired, interleaved runs: 0.77 to 0.84 in one process, 0.66 to 0.82 in N processes), replays
  byte-identical. Left over: keep the worker's one process per slot (it buys those 11% at 16 slots, and a
  timed-out match is killed alone). The one deflate left (level 8) is about a fifth of a match; another
  level changes replay bytes, so that is a patch. Re-measure on the fleet's Linux hosts: WSL2 here was too
  noisy to resolve less than 10%.
- [ ] **The `unsafe` in the fuel logic.** `crates/runner/src/vm.rs` reaches the store through a raw pointer to
  re-arm a suspended bot's fuel each turn. Checked against the Wasmtime 51 source on 2026-11-27: no safe API
  can do it. When a fuel slice runs out, Wasmtime refuels and yields without calling the embedder, and runs
  nothing on resume, so neither host calls, call hooks nor epoch callbacks can read or re-arm fuel there. The
  SAFETY argument on `UnitVm::store_mut` now names the three Wasmtime behaviours relied on,
  `crates/runner/src/vm/tests.rs` pins them turn by turn, and the pin in `crates/runner/Cargo.toml` says to
  re-check both on a bump. Left over: ask upstream for an out-of-fuel callback with store access (like
  `epoch_deadline_callback`), then drop the `unsafe`.
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
- [x] **Stack-overflow depth was architecture-dependent.** Done: the runner counts the stack in the module
  (`crates/runner/src/stack.rs`, the stack height limiter of Parity's `wasm-instrument`), and a call deeper
  than `stack_limit` units traps the same way on every CPU. Wasmtime's native stack is sized at 64 bytes per
  unit for up to `MAX_STACK_UNITS` (twice the rule), four times the widest x64 frame measured. The reference
  bots need at most 333 units (the squad bots; `rusher` 242, `holder` 249, `utility` 261, bisected on
  `crossfire` and `spillway`), and it costs them 0.9 to 1.8% fuel per turn (97 wasm games on `crossfire` and
  `quarry` that stayed identical).
  Was: a bot that recursed until it trapped faulted where the native stack ran out, which depends on
  Cranelift's frame layout for the ISA: on x64 16,368 levels of a thin frame, 3,273 with 16 i64 locals and
  1,818 with 16 v128 locals (`crates/runner/tests/stack.rs`). Left over: run that test on arm64 when
  `determinism.yml` runs (section 4) to confirm the margin there. A frame that Cranelift makes much larger
  than its stack units (many recomputable values kept alive across a call) could still run out of native
  stack first.
- [x] **`memory.copy`/`memory.fill` cost fuel per byte.** Decided: keep Wasmtime's 1 fuel per byte. Free bulk
  operations would let one tick cost unbounded CPU; Wasmtime's per-byte cost is an integer, so the only options
  are 0 or at least 1; a hand-written copy loop costs about the same. `docs/RULES.md` says that one huge copy is
  paid for out of the following turns. Was: consider setting `Config::operator_cost` so a 100 KB `memcpy` does
  not consume a whole tick.
- [x] **`lead` and `dist` in the Rust SDK overflow** on extreme inputs (a bullet speed near 1). Done:
  `dist`, `len` and `lead` compute in 64 bits and saturate at the i32 range, `dist2` saturates at
  `i64::MAX`; `sdk/cpp/ibc.h` does the same. `sdk/rust/tests/geom_overflow.rs` checks 1.3 million inputs the
  old code handled (216,317 of them `lead` calls) for the same answer, and 0.5 million that overflowed
  against an exact i128 model; the goldens did not move. 3 of the 600 `lead` inputs of
  `sdk/cpp/tests/geom_test.cpp` had wrapped, so its checksum changed; `sdk/rust/tests/cpp_port.rs` now
  computes those checksums. The checks cost `rusher` 1.4% more fuel (crossfire, seed 1). Left over, outside
  these helpers: `cos_sin` (`angle + 16384`) and `offset` (the final addition) still overflow on extreme
  inputs in a Rust debug build.
- [x] **`ibc build --debug`** and bots that print to stdout have never been exercised. Done (docs/CLI.md,
  "Debug builds and bot output"): a debug build plays, but at a median 6.9 times the fuel per turn and out of
  fuel on 29% of its turns; an overflow panics, the message lands in the unit log and the unit faults.
  stdout, stderr and `ibc.log` share the unit log in order, the cap truncates with its notice, other fds get
  `EBADF`: `crates/runner/tests/wasi.rs`, and `crates/cli/tests/debug_build.rs` end to end (skipped without
  the wasm32-wasip1 target).

## 3. Job system and platform

- [x] **Deployment files have never been run.** Done (2026-11-27, Docker Desktop 28.3 on Windows):
  `docker compose up` built both images, served the web front end and viewer, registered two trusted workers
  and played ranked series for four teams. `deploy/cloud-init.yaml` (only its variables filled in) booted a
  fresh Debian 12 with systemd 252 (an imported WSL distro, not a cloud VM): it downloaded and checked the
  archive, installed and started `ibc-worker.service` (dynamic user, `LoadCredential`, full hardening),
  registered with the untrusted token, emptied it, and played 12 matches. Fixed on the way: compose set
  `IBC_DISTRIBUTION` to an empty string, which the coordinator refuses to start with. Left over: the archive
  was packed by hand from the image's glibc binaries, not the musl one `cargo xtask dist` makes (section 4);
  `ibc-coordinator.service` has not run; systemd inside a Docker container cannot test `LoadCredential`.
- [x] **Drain on SIGTERM has only been tested through the drain file.** Done on Linux: a unit test sends real
  SIGTERMs to the binary (one drains, two abort), and `docker stop`, `systemctl stop` and the drain file all
  end in exit 0 with no restart. A job granted by a long poll that was open when the drain began is now
  released unstarted; an idle worker still takes up to `--poll-ms` (25 s) to exit.
- [x] **`--allow-runner-fetch` and side-by-side runner directories have no test.** Done:
  `crates/worker/tests/runners.rs` (Windows and Linux) covers a side-by-side runner, one naming another
  patch, no download without the flag or for another platform, a single download then reuse, a wrong hash, a
  fetched binary naming another patch (deleted), and a patch that is not a plain directory name (nothing
  downloaded or written).
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
- [ ] **SQLite and local disk only.** Fine for one box, and decided (2026-11-27) to stay for the prototype.
  `crates/coordinator` keeps SQL in `src/db/` and blobs behind a trait for a later Postgres and S3 backend.
  Not needed until a second coordinator or off-box storage is.
- [x] **Backups and retention.** Done: `ibc-coordinator backup`, `prune-replays`, `deploy/backup.sh` (unit 16),
  per-team submission retention (unit 11). Was: Document and automate copying the SQLite file and blobs. Add a replay
  retention policy: replays can be regenerated from (patch, map, bots, seed), so unranked and bench replays
  need not be kept forever.
- [x] **Abuse limits.** Done (unit 11): per-token and per-IP request limits, uploads per hour, module size,
  429 with `Retry-After`, suspend/ban for teams and users. Was: Upload rate and size per team, total submissions kept per team, API rate limits.
- [x] **Cost prior in the budget governor.** Done: the prior is 2 core-seconds per game (about twice the
  longest legal match below, for slower cloud vCPUs) and the floor 0.05 (0.25 held a ladder of today's bots to an eighth
  of the fleet). Measured 2026-11-27 with `bots/burner` (every unit spends its whole fuel every tick;
  all 14 maps, 4 seeds, both sides, native `ibc` on an i7-12700H): 0.53 core-s per game mean, 0.66 max doing
  nothing, 0.47 / 0.66 on top of `forcer`; the longest legal match projects to about 0.9. Cache-missing code
  reaches 20 to 31 core-s. The bound only holds once host calls are priced (section 2). Was:
  `series_per_batch` fell back to 10 core-seconds per game when it had no measurement, against a measured
  cost of about 0.03, with a floor of 0.25; unit 9 made it schedule at least one series per round until games
  are timed, and devstack uses a 30 s interval.
- [x] **Plagiarism and duplicate detection.** Decided (2026-11-27): no near-duplicate detection. Teams are
  responsible for keeping their own source private; the platform never shows source, never serves one team's
  compiled module to another (teams list only their own submissions, public replays are redacted per viewer)
  and runs every match server-side. Identical wasm across teams is still flagged (unit 11).
- [ ] **Accounts:** emails are not verified, so register the `IBC_ADMIN_EMAILS` accounts before the site is
  public; login throttling is per email only (put per-IP limits in the reverse proxy).
- [x] **Any worker token can read any blob.** Done: a worker reads only the bots, map and runner of a job it
  holds a live lease on (anything else is 403: the worker fails the job as retryable) and uploads only while
  it holds a lease (otherwise 409 `lease_lost`); the admin is unrestricted (`docs/JOBS.md`, `blobs`). Still
  open: a worker holding any lease can upload any content up to the blob size limit. Was: `GET /v1/blobs`
  accepted the admin or any worker token for any blob by hash: every team's compiled module, every map
  (hidden tournament maps too) and unredacted replays.
- [x] **Double elimination has no bracket reset.** Done: when the losers-bracket champion wins the grand
  final, a reset between the same two teams decides (`docs/api/tournaments.md`). Was: The grand final was
  played once, so the winners-bracket champion was out after its first loss.
- [x] **Docker images have never been built.** Done (2026-11-27): both targets build from a clean export of
  `HEAD` (coordinator 53 MB, worker 73 MB) and run under compose (above).

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
- [x] **CI on PR 15.** It did run, on both commits, and passed, but the PR was merged 26 s before the last
  `check` job finished (`main` has no required checks; a private repository needs GitHub Pro for them), and
  the worker's real-runner tests had skipped: CI never set `IBC_RUNNER`, so `tests/e2e.rs` "passed" in
  0.00 s. CI now sets it, those tests fail instead of skipping when `CI` is set, and `determinism.yml` also
  runs them, on all four platforms.
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
- [ ] **Per-language allowances.** One table per language holding the fuel multiplier and a memory multiplier,
  ideally in `rules/` so it is hashed and identical for local runs and the ladder. Today fuel multipliers are
  coordinator config (`--language`), recorded per bot in the replay header (`BotInfo.fuel_multiplier`). A
  memory multiplier would scale the `memory_bytes` cap in the runner and in the upload check.
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
- Ladder cadence. Ranked series are now 3 games (sides alternate per game), a round every 600 s plus fill-ups
  whenever the workers have less than 120 s of work queued, at most 5 series per team per 15 minutes (up to
  10 when drawn as an opponent). Rematches are allowed and weighted down, not capped per day. The cap and
  backlog are first guesses: measure storage per day on a real ladder before the event.
- Team shared memory size (64 bytes now). (Sightings and sounds are now shared automatically.)
