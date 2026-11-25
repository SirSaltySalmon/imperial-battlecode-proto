# Vision feedback

Review of [`vision.md`](vision.md) against the tree as of 2026-11-25 (foundation, first unit wave, and the job system on `main`). Nothing here is implemented. Numbers are the current `rules/*.toml` and the coordinator defaults.

A match is a pure function of `(patch, map, bots, seed)`. Almost every gameplay line in the vision changes that function, moves `fixtures/golden`, and bumps the engine version per [`docs/release.md`](../release.md). A number that already lives in TOML changes the ruleset hash only. A new thing bots can see is an additive ABI change ([`docs/ABI.md`](../ABI.md)): new ids at the end, new fields at the end of a record. Changing what an existing id means is a silent break for every bot already written, even when the struct layout stays put.

`docs/TODO.md` already tracks several of these as open work (objective rarely ends a round, overtime mirrors, no tournament runner, no web UI, abuse limits, map editor, handbook generator). This note says what the vision adds on top, and where the vision contradicts itself or the engine.

## Already in the game

These lines of the vision describe the current rules. Leave them alone unless a later item explicitly changes a number.

- First to 7, two halves of 6, sides swap, credits reset at half. `rounds_to_win = 7`, `rounds_per_half = 6` in `rules/core.toml`. The economy worked example in `docs/rules/economy.md` is pinned by an engine test.
- Attackers win by wiping the defenders or by detonation. Defenders win by wiping the attackers before a plant, by defusing, or by the action timer. After a plant, wiping the attackers does nothing: `World::step_objective` in `crates/engine/src/world.rs` only ends a planted round on defuse, fuse expiry, or no defenders left.
- The round stops when it is decided. `end_round` switches to `PHASE_ROUND_END` and `step` does not run movement, shots, or intents there. `round_end_ticks = 2` is a pause with no actions, and the parser refuses a value below 1. Post-round kills cannot farm credits.
- Facing is rate-limited (`turn_rate = 20000`, about 110 degrees per tick). Shots are clamped to the vision cone. One move, one facing, and one primary action per tick.
- Running makes footsteps; walking at or below `walk_speed` does not. Shots, plant, defuse, and ability use are audible. Heard positions are the tile centre. A bot builds its own estimate from `T_SOUNDS`. The decoy already forges step and shot sounds.
- Spread grows with speed and recoil. Bullets leave on the tick the wind-up ends, and spread uses the speed on that tick, so a unit can move during the wind-up and pay for it. The formula is in `crates/engine/src/combat/firing.rs`. There is no random draw.
- Smoke, flash, fire, recon, barrier, dash, decoy, and heal exist, with a `delay` field for arming. Barriers are segment effects in `T_EFFECTS`, distinct from map tiles. Thrown devices are visible while they arm. Flash and fire hit both teams.
- Draft is one tick with extra fuel (`fuel_first_tick_mult = 10`). Draft memory is 1024 bytes, writable only in `PHASE_DRAFT`, readable after. Buy phase fuel is 4× for `buy_ticks = 3`. Team memory is 64 bytes.
- Abilities with `price = 0` refill to `max_charges` every round. Any other ability is bought and the charges survive death and the side swap (`abilities::on_round_start`).
- A dead unit drops its weapon (`combat::on_death`). A unit that is dead when the round ends starts the next round on the sidearm. A survivor keeps weapons and armour.
- Wasm bots see a fogged snapshot, a fuel meter they do not control, and a WASI subset with no filesystem or network (`crates/runner`, `crates/coordinator/src/wasmcheck.rs`). A trap faults that unit for the rest of the match; the opponent is a different instance. Fuel left at the end of a tick is discarded.
- Replays store every accepted input and no wasm bytes (`crates/replay`). Verification re-simulates from those inputs. Public `GET /v1/matches/{id}/replay` serves that file with no token.
- Submissions are kept and pinned onto a series when it is created. A later upload does not change a series already queued. `POST /v1/admin/ranked-batch` and a background scheduler (`IBC_RANKED_INTERVAL_SECS`, default 0, so off) pair live teams with Weng-Lin. An `elo` system is also implemented. Unranked series are capped at 100 games per team per 24 hours. Tournament jobs are a priority class that is served first. Untrusted results are spot-checked; a mismatch quarantines the worker.
- Maps are text (`.ibcmap`). `ibc-maptool` lints and measures them. `ibc run` already accepts any map file, so local custom maps work. `ibc-bench` is the balance tool. CI does not start on docs-only or viewer-only diffs (`.github/workflows/ci.yml`).

## Decide these before changing code

The vision states two designs at once in a few places. Pick one in `vision.md` so the implementation does not guess.

**Tournament shape.** "Seeded single or double elimination" and "all teams play each other on all maps" are different events. A 32-team round robin on 10 maps is hundreds of games per pairing times the pair count; a double-elimination bracket is one path through the field. The hour of stopped queueing cannot hold either, and the vision already says the bracket continues after ranked resumes. Write down: field size, single or double elimination, how many maps each pairing plays, and whether "all maps" means the whole pool or a short hidden list. The coordinator can pin bots and prefer tournament jobs today. It cannot build a bracket, hide a map list, or pause the scheduler.

**What a split overtime is.** Regulation is already first to 7. Overtime is already capped: `overtime_max_pairs = 3` in `rules/economy.toml`, then a tie-break (rounds, then kills, then damage, then a coin flip on the seed). The vision's "one overtime, sides swapped, do not continue like Valorant" is `overtime_max_pairs = 1` plus the existing tie-break. Say what should happen when that single pair splits. A coin flip is a bad final. The ladder already understands draws (`outcome: "draw"` is not rated as a win). A tournament needs an explicit draw rule because a bracket cannot seat a draw.

**Draft information.** Today `A_DRAFT` is simultaneous, blind, and unlimited: every unit may take the same kit, including kit 0 which has no abilities (`abilities::draft`). Units are dead at `(0, 0)` during that tick, and `T_ENEMIES` only lists living units you can see, so the enemy composition is hidden until contact. Allies see each other's kits through `T_ALLIES`. A Valorant-style select usually reveals both teams, picks in order, and forbids duplicates on a team. There are six real kits and maps of 3 to 5, so unique-per-team fits only if duplicates are disallowed and the plain kit is not required. Bans need more kits than seats. Sequential picks need several draft ticks and a turn order in `World::step`, which is core match flow, not a seam in `crates/engine/src/abilities`.

**"Random" spray.** Spread is a pure function of weapon, speed, and recoil, and `docs/RULES.md` tells bots they can compute it. The match seed is visible (`MatchInfo.seed_lo`). A seeded noise term in `firing.rs` would still be reproducible by any bot that runs the same integers, and it would be hashed or replays would lie. Unpredictable-to-a-strong-bot spray does not exist inside a deterministic public ruleset. The vision's actual goal, punish shooting on the move, is already `move_spread`. If that number is too small, raise it in `rules/weapons.toml` and look at the bench. Do not add an `f64` draw.

**Detonation and the next round's guns.** Detonation currently ends the round and damages nobody (`EVENT_DETONATE` then `end_round`). Survivors keep their rifles. A blast radius that kills, combined with the existing death rule, means anyone inside loses the weapon and anyone outside keeps it, on both teams. That is a large economy lever: a defender who runs off site keeps a rifle after losing the round, and an attacker who plants on top of their own team throws the next buy away. The radius has to be chosen against site size and rotation time (`ibc-maptool analyze`), and it has to be long enough that escaping is a real choice. This is the same problem as `docs/TODO.md` section 1: in the last measured pool, 0 of 390 rounds ended by detonation or defuse.

**Who may download a replay, and when.** The vision asks for both "no decompiling another team's bot" and "match data downloadable so participants can watch and analyze". Public replays contain every `act` and every memory write. That is the policy, in log form, without the wasm. Wasm bytes are not in the replay; blob download of a module is worker-authenticated. A dishonest worker with a registration token can still save every module it runs. Obfuscation will not survive that. The decision is access timing: own matches only, a delay, or open. Open is the better learning tool and the better scouting tool. Collusion (two accounts trading results) is not detected. The daily rematch cap in `crates/rating/src/matchmaking.rs` (2 series per opponent) is the only brake, and it applies to ranked pairing, not to unranked challenges.

**Signature charges.** `price = 0` means "refill to max every round" (dash, recon, decoy, heal). The vision means "one free charge each round, unused charges stay, further charges are bought". Those are different economies. Dash on lancer and kindler is two free casts a round today and would become a bankable resource. Extra signature charges need a price; the shop sells one item per kit slot, not per ability (`rules/economy.toml`, `crates/engine/src/economy/shop.rs`). Add a field on `[[ability]]` rather than overloading `price`.

**Series length.** The vision wants 7 games on 3 to 5 maps. The coordinator default is `IBC_RANKED_GAMES = 2` (`crates/coordinator/src/main.rs`). `plan_series` (`crates/rating/src/series.rs`) picks that many distinct least-used maps and does not repeat a map until the pool is exhausted. Seven games on three maps means repeats. Each game already plays both sides (half-time swap). The A/B engine slot also alternates, and the config comment asks for an even game count so neither team gets an extra slot-A game. Seven gives one team that extra slot. Six or eight games keeps the slot count even. Three to five maps can still be the set those games are drawn from.

**Rating math.** Each finished game appends a `game` event and Weng-Lin updates from win, loss, or draw (`crates/coordinator/src/db/ratings.rs`). Map difference across a series is felt because each map is its own update. A second term for "maps won minus maps lost", or a single update per series, would either double-count or throw away per-map results. Elo is available as `IBC_RATING_SYSTEM=elo` and recomputes the ledger; it has the same per-game outcome shape. Prefer keeping per-game Weng-Lin. The rating gap between the teams is already an input.

**Rank labels.** Iron through Radiant, with Emerald inserted and Ascendant listed above Immortal, is a display scale. Valorant's order is Ascendant, then Immortal, then Radiant. Emerald is a different game. Put thresholds on the displayed rating in the ladder response. Do not matchmake inside brackets. A hard bracket edge recreates the "freeze by having nobody to play" problem the vision wants to avoid, and the current matcher deliberately allows a few uneven games so the graph stays connected.

**Friendly fire for bullets.** Still an open decision in `docs/TODO.md`. Flash and fire already hit your own team. The vision does not say. Bullets currently pass through teammates (`docs/rules/combat.md`). Pick one before more abilities land, because traps and blasts will copy whichever rule bullets use, or they will invent a third.

## Gameplay changes that touch the engine

Ordered so that a rules-only patch can be measured before a mechanics patch. One change at a time, then `ibc-bench` on the whole bot pool and both sides. The reference bots do not draft, buy, or use abilities (`docs/TODO.md`), so a league run today cannot tell whether an ability "turns a round". Growing the bot pool comes before tuning the new toys, or the numbers will move for the wrong reason.

### Rules data only

| Vision | Current | Where |
|---|---|---|
| One overtime, both sides, then stop | Up to 3 pairs, then tie-break | `overtime_max_pairs` in `rules/economy.toml`. `max_ticks` math in `docs/rules/economy.md` gets shorter, which is safe. |
| Plant and defuse are long; plant is faster than defuse; rotating is viable | Plant 4, defuse 7, kit defuse 4, fuse 40, action 120 | `rules/core.toml`. Kit defuse is equal to plant, so "faster than defuse" is false for a defender who bought a kit. Lengthening phases must raise `max_ticks` (3000). A match that hits `max_ticks` is `unfinished` and is not rated. |
| Most abilities have a startup | Barrier, dash, decoy, and heal have `delay = 0` | `rules/abilities.toml`. |
| Moving and shooting is heavily punished | `move_spread` per weapon | `rules/weapons.toml`, after a bench look, not a new RNG. |

Goldens move. The engine patch version moves. The ABI does not.

### Memory phases

The vision: draft memory stays as it is; buy memory is smaller, resets every round, and is the only scratchpad in the buy phase; team memory is the smallest of the three and is readable and writable only in the action and planted phases.

Today `mem_write` allows team memory in every phase (`crates/engine/src/host.rs`). There is no buy scope. `MEM_TEAM = 0` and `MEM_DRAFT = 1` are frozen.

Code:

- Add `MEM_BUY` at the end of the scope ids in `crates/abi/src/lib.rs`. Add `buy_mem_bytes` at the end of `CoreRules` (and the TOML key in `rules/core.toml`). Old bots keep working if they ignore the new scope.
- Store `buy_mem` on `World`, zero it in `start_round`, and hash it next to `draft_mem` and `team_mem` in `World::state_hash`. Unhashed memory will make replays verify when the bytes differed.
- In `mem_write`, accept `MEM_TEAM` only in `PHASE_ACTION` and `PHASE_PLANTED`, and `MEM_BUY` only in `PHASE_BUY`. Reads can stay open so a unit can see what it wrote earlier.
- Mirror the scope in `sdk/rust` and `sdk/cpp`. Both SDKs document the two scopes today.
- 64 bytes may be too small for the coordination the vision also asks for. The size is a rules number bots can read. Change it only after a bot has tried to play inside 64 bytes. Buy-phase length is 3 ticks; a planning scratchpad with a 3-tick budget will not get used. If buy memory is real, `buy_ticks` has to grow with it, and `max_ticks` with that.

This is a behaviour change for team memory even for bots that never call the new scope: a write during buy that works today will return `ERR_NOT_ALLOWED`.

### In-match history

`T_EVENTS` is cleared at the start of every `World::step`. Bots see the running score on `MatchInfo` and the last tick's kills, plant, defuse, detonation, and round end. They do not get a round log unless they stored one themselves.

If "match results and history visible at all times" means the website, the public API already has ladder, series, matches, and replays, and the missing piece is a frontend (`docs/TODO.md` section 3).

If it means the bot, add an additive table (a `RoundInfo` row per finished round: winner, reason, scores). The data is already `World.rounds`. Do not widen `T_EVENTS`; that table is defined as the previous tick.

### Detonation blast

In `step_objective`, before `end_round` on fuse expiry, kill living units whose centre is within a new `blast_radius` of the charge. Death already drops the weapon and already strips the loadout next round, so "lose the gun if you do not escape" falls out of the existing carry rule. Survivors outside the radius keep their guns into a round they lost.

Put the radius in `rules/core.toml` and on `CoreRules` (end of the struct). Hashing already covers unit alive flags and drops. Document the economy worked example again; it assumes detonation does not change who is alive. `crates/engine/src/world.rs` is a core file. A seam would be cleaner, and there is not one for "the fuse reached zero".

### Traveling utility, traps, more abilities

Smoke, flash, fire, recon, and decoy do not fly. On the tick they are used they appear at the target, or short of the first wall, and then wait for `delay` (`docs/rules/abilities.md`). Bullets are the only moving projectile.

A throwable that is slower than a bullet is a second user of the projectile step in `crates/engine/src/combat/ballistics.rs`, with its own speed, a team, and a payload that arms on arrival. It should show up in `T_PROJECTILES` (fog already applies). One projectile path, shared by every thrown ability. A special travel loop per ability will drift.

Traps are a new effect id, appended after `EFFECT_DASH` in the ABI, implemented in `crates/engine/src/abilities`, hashed with the other effects. Trigger, arming, friendly-fire, and whether they block movement are the design. There is no trap code.

"Many more abilities" splits in two. A new row that reuses smoke, fire, barrier, and the rest is a TOML edit and a ruleset-hash change. A new effect kind is engine code plus an ABI id. A bytecode of ability behaviours would be a new system. Do not build it until the effect list from real games has settled. The eight current abilities have never been played by a bot that chose to use them.

Player-placed walls are the barrier. `los` and `raycast` are documented as the static map only; barriers are queried through `T_EFFECTS`. Bots already can tell a barrier from a `#` tile. Changing `los` to see dynamic tiles would change the meaning of a frozen host call. If a future wall must be a tile, add a new call and leave `los` on the static map.

Startup on the abilities that have `delay = 0` is the TOML table above. The arming rule (visible, one tick to react when `delay = 1`) already matches the vision's "startup".

### Draft, if it becomes agent select

Minimum engine change for "different kits" as unique-per-team: `abilities::draft` rejects a kit a living teammate already took. Still one simultaneous tick. Still hidden from the enemy.

A revealed, ordered select needs:

- more than one draft tick, and a rule for who picks (unit id order will bias seat 0; say so or randomise from the seed);
- enemy kits published without line of sight, which means a new table or a flag, because `T_ENEMIES` will not show a unit you have not seen;
- bans, if they are in scope, as more draft actions (`A_DRAFT` is frozen; a ban is a new action id);
- enough kits that a team of 5 has a real choice after unique picks.

`UnitInfo.kit` is already public on any unit you can see. The gap is the pre-contact composition, not the field.

## Platform changes

None of this belongs in `crates/engine`. The match function stays the same.

### Ranked queue and the shape of a series

Turn the scheduler on in deployment (`IBC_RANKED_INTERVAL_SECS` is 0 by default, so a stock coordinator never queues ranked). The code path is `App::ranked_round` in `crates/coordinator/src/lib.rs`. Teams with a live submission are always eligible. There is no way to clear the live slot, only to replace it (`set_live`), so a team that has uploaded cannot dodge the queue by deleting the bot. That already blocks "freeze my rating by having no bot". `on_inactive` in `crates/rating/src/weng_lin.rs` is never called. Wire it only if a team should grow uncertain while the season runs and they keep playing; it is the wrong tool for a team that is actively paired.

`plan_series` has to learn a two-step plan if the vision's shape wins: choose K maps (3 to 5) from the pool, then deal 6 or 8 games across them with replacement, still least-used-first so the pool stays level. `IBC_RANKED_GAMES` becomes that game count. `BudgetConfig.games_per_series` in `crates/rating/src/budget.rs` defaults to 5 and is overridden by the coordinator; update the governor so a 7-game series is not costed as 2 or 5. The prior of 10 core-seconds per game is already known to be far above the measured ~0.03 (`docs/TODO.md`).

Rank labels are a function of `RatingSystem::display` in the `/v1/ladder` payload (`crates/coordinator/src/api/public.rs`). Provisional teams still need a product decision (the number starts near 763 under Weng-Lin).

### Unranked against an old build

`create_series` loads each team's live submission only (`crates/coordinator/src/db/league.rs`). The archive exists (`list_submissions`, `select_live`). To play "their submission from Tuesday", the request body needs submission ids, the coordinator must check the caller is allowed to spend quota on that pair, and `check_wasm` must still accept the bytes on the current patch. After an ABI or engine break, old modules fail validation. The grace-period rule is undecided (`docs/TODO.md`). Unranked does not move rating, so the farming risk is compute, which the daily quota already bounds. It does not bound "I want every historical blob of every team".

### The weekly tournament

New coordinator behaviour, on top of the existing `tournament` job class:

- A pause flag the scheduler and `POST /v1/series` honour for one hour. In-flight ranked games should finish. Voiding them is the patch-boundary question already open in `docs/TODO.md`, and the vision does not ask to void them.
- A snapshot of every live submission at the pause. Series already pin submissions, so the lock is "read live once, ignore `select_live` after". Uploads during the hour can be stored and refused as live until the lock ends.
- A bracket table: seeding from the ladder, single or double elimination, one series per edge, games equal to the hidden map list. This is the piece that does not exist.
- Maps with a visibility state. `pool_maps` is one list and ranked uses all of it. Tournament maps have to be excluded until the hour ends, then inserted. Run `ibc-maptool lint` and `analyze` before they enter the ranked pool. Three current maps are lopsided with the bots we have; a surprise map can be worse.
- The hour is not the runtime. Tournament priority already jumps the queue. Ranked resumes by clearing the pause. Budget the bracket: it shares the worker fleet with the ranked scheduler, and `docs/TODO.md` notes the runner scales poorly inside one process.

### Cheating and limits

| Threat in the vision | What the code does | Gap |
|---|---|---|
| Decompile an opponent | Replays have inputs. Wasm is fetched by workers. | Decide replay publication. A leaked worker token is a copy of every module that worker ran. |
| Inject code into the match | Import check, memory cap (`max_memory_bytes`), no host filesystem. | Keep it. Engine panics caused by a hostile module are bugs, not a policy. |
| Cheat the fuel counter | Wasmtime fuel, host charges in `crates/abi`. The bot can read `fuel_remaining` and cannot set it. | `?language=` on upload selects the multiplier (`manage::upload_submission`). Rust and C++ are both 1. The server must classify the module itself before any language is above 1. The `unsafe` fuel re-arm in `crates/runner/src/vm.rs` is a Wasmtime-internals dependency (`docs/TODO.md`). |
| Crash the game and gain the round | The faulted unit idles. The job retries on the same seed. A bad result from an untrusted worker can be re-run. | Host OOM from engine state is the remaining crash. Caps on effects and projectiles belong with traps and throwables. |
| Freeze rating by not fielding a bot | Live submissions stay in the pool; the slot cannot be emptied. | Scheduler default off, so in the default deploy nobody is paired and every rating is frozen. Turn it on. Do not add an opt-out. |
| Upload and match floods | Wasm size cap, blob size cap, 100 unranked games per day, ranked is server-scheduled. | No upload rate, no general API rate limit, no cap on stored submissions (`docs/TODO.md`). Add those on the coordinator, not in the engine. |

There is no account ban. `quarantine_worker` is for compute hosts. A moderation action is a column on the team, a filter in `matchmaking_pool`, and an admin route next to the worker routes in `crates/coordinator/src/api/manage.rs`. `/metrics` already exposes queue and worker counts for the "performance" half of a moderation view. The terminal itself is UI, in the same bucket as the missing website.

### Participant surface

Local runs, the HTTP API, readable wasm rejection text, and replay download exist. The website does not: sign-in, team page, upload form, ladder page, embedded viewer. Sign-in is undecided. Agent-facing errors should stay machine-readable (`code` plus `message`, as the API already returns) and should include the quota numbers in the body, which the unranked 429 already does.

`fuel_mult` is returned on upload. Keep telling the caller which multiplier they received once the server assigns it, so an agent can see that it did not get to pick.

## Development experience

**Glossary.** The vision's "match" is the coordinator's "game", and the engine's "match" is one map from draft through the final score. "Series" is the set of games. "Slot" is which wasm sits in team A. "Patch" is engine version plus ruleset hash plus Wasmtime version plus ABI version. Agents will mix these. A short glossary, generated or hand-written once and linked from `CLAUDE.md`, is the highest-leverage doc in the vision. This file is a review and should not become that glossary.

**Docs computed from sources.** `rules/*.toml` and `crates/abi/src/lib.rs` are the sources of truth. `docs/RULES.md`, `docs/rules/*.md`, and `docs/ABI.md` are hand-kept and will drift (the abilities page still says dash has no constant; `EFFECT_DASH` is in the ABI). The handbook generator in `docs/TODO.md` section 6 is the right project. Generate the tables. Keep the prose that explains decisions. When a feature is removed, a short `DEPRECATED.md` entry is a policy choice and needs no machinery; do not build an archive tree.

**Tests only when relevant.** CI already skips itself when only docs or the viewer change, and the wasm golden job is filtered again inside the workflow. `cargo xtask check` still runs the crate tests when any crate changes. A finer "this test cannot be affected" selector will miss the cross-crate breaks this repo actually has (sim versus wasm fuel, goldens versus rules). Keep the workflow filters. Do not skip in-crate tests.

**Bench on every balance change.** `ibc-bench` is ready. The balance figures in `docs/TODO.md` predate the current weapons, abilities, and maps. Re-measure before acting on them. The vision's ability-weight goal is a bench target: share of rounds swung by an ability, reported with seed count and an interval, against the whole pool.

**Human play and the buy menu.** The viewer plays replays. The combat lab (`viewer/src/lab.ts`) scrubs weapon fans. Neither sends `A_MOVE` or `A_BUY`. The data for a buy menu and a teammate panel is already in the engine JSON and the ABI (`T_ITEM`, `T_WEAPON`, `T_ABILITY`, `T_KIT`, ally `UnitInfo`). A human session is a client that turns keys into intents once per tick and clamps facing with the existing `turn_rate`. "Realtime" is that loop on a wall clock. "Turn-based" is the same loop stepped by a key. The clock stays out of `crates/engine`. Debug drawings still never reach the viewer (`docs/TODO.md`); fix that before building a play mode on top of empty overlays. Core rules are also hardcoded in `viewer/src/vision.ts` and will lie after a balance patch.

**Maps.** Authoring a local map is a text file plus `ibc-maptool lint`. What is missing is the browser editor in `docs/TODO.md` section 6, and a way for a participant to upload a map into their own unranked games. Ranked pool insertion should stay an organiser action, especially if tournament maps are hidden first. `SiteInfo` is still a bounding box; L-shaped sites are lint-rejected rather than given a guaranteed plant tile.

**Moderation and ops.** Covered above. Deployment files have never been run (`docs/TODO.md`). A moderation terminal on top of a coordinator that has not booted is the wrong order.

## What the vision does not mention, and the code will force

- **Versioning.** Memory scopes, blast radius on `CoreRules`, a new effect id, and a round-log table are additive ABI. Phase gates on team memory, unique kits, blast kills, and traveling projectiles change state hashes. Ship them as separate engine minors so the league table has a cause. `on_balance_patch` exists and is not called; a balance patch currently leaves every sigma as it was.
- **`max_ticks`.** Longer buy, plant, defuse, or fuse, or a longer overtime, can push a match into `unfinished`. Unfinished games are dropped by the rater. The check that regulation-plus-overtime fits in `max_ticks` has to move with the numbers.
- **Native sim has no fuel.** Reported balance runs that use `ibc-sim` disagree with wasm when a bot exhausts its budget (`docs/TODO.md`). A buy-phase planner that spends the 4× budget will show up here first.
- **Pathfinding cost.** A distance field can exceed a tick of fuel on the larger maps. New abilities do not fix that. A host path helper would be an ABI addition of its own.
- **Second weapon and weapon cooldown** are invisible on `UnitInfo`. A human buy menu can show them from the rules tables; a bot still cannot see its own cooldown. That additive field is in the TODO and will be felt as soon as anyone plays more than the rifle.
- **Language multipliers and the Python SDK** are the fuel-cheat deadline. Until the server classifies modules, do not configure a language above 1.
- **Patch boundaries** void running jobs today. A weekly tournament that straddles a balance patch needs the "finish the series on the old patch" decision, or the bracket dies at the patch.
- **Copying a bot from replays** remains possible under any source-hiding scheme. The counter is a delayed or private replay, accepted as a cost to the "analyze your games" goal, or an open replay, accepted as scouting.

## Suggested order

1. Edit `vision.md` so the open decisions in the second section have a single answer. Especially the bracket versus round robin, the split overtime, whether enemy kits are public, and whether spray stays computed.
2. Re-run `ibc-bench` on the current pool. Then, one TOML change at a time, lengthen plant, defuse, and fuse, and set `overtime_max_pairs` to 1, if the measurements still say the objective does not matter.
3. Grow the bot pool until abilities and the objective are actually played. Otherwise later patches have no baseline.
4. Memory phase gates, buy memory, and a round-log table. Additive ABI, one patch, goldens re-recorded.
5. Signature-charge field, then the detonation blast. Separate patches. Each one rewrites the economy example.
6. Traveling projectiles on the existing bullet step. Traps after friendly fire for area effects is decided.
7. Turn the ranked scheduler on, set the series shape, add display tiers. Then the tournament lock and hidden maps. Then bans and upload limits.
8. Viewer: rules from the replay header, debug drawings, buy menu and teammate panel, then a local human client. Map editor after `ibc-maptool` is the lint the editor calls. Handbook generator before asking agents to keep prose docs in sync.

Steps 2 through 5 change the game everyone is training against. Step 7 can proceed in parallel on `crates/coordinator` because it does not change a match. Step 8 can proceed on `viewer/` for the same reason, until the human client needs a new intent the ABI does not have.
