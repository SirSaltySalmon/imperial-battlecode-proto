# Glossary

The exact meaning of the words used in this repository, its docs, the API and the replays. People and AI agents
should use these words this way and no other; where two words name the same thing, the preferred one is first.

Terms marked **(landing)** are decided in `docs/informal/vision.md` but not yet fully in the code on `main`; the
definition given is the decided one, and the entry says what the code does today. Numbers quoted here are the
current values for orientation only; the source of truth is `rules/*.toml` (see `docs/generated/rules.md`).

## Time inside a game

**Tick.** One step of the simulation. Every living unit's bot runs once against the same start-of-tick snapshot
and queues intents; then the engine resolves them all together (`docs/RULES.md`, "A tick"). Tick 0 is the draft.
`MatchInfo.tick` counts from 0. A game ends by `max_ticks` at the latest.

**Turn.** One bot instance's share of a tick: from the moment the host resumes it until it calls `end_turn` or
runs out of fuel. Each unit gets at most one turn per tick, in the turn order (teams alternate, and the unit
slot that goes first rotates every tick). `Bot::turn` in the Rust SDK is one turn. Never use "turn" to mean a
round or a tick.

**Phase.** What a tick is for (`MatchInfo.phase`): draft, buy, action, planted, round end, match end
(`PHASE_*`).

- **Draft phase.** Tick 0 only. Units pick a kit (`A_DRAFT`) and may write draft memory. Extra fuel
  (`fuel_first_tick_mult`).
- **Buy phase.** The first `buy_ticks` ticks of every round. Units stand at spawn and buy (`A_BUY`); purchases
  apply at once. Extra fuel (`fuel_buy_mult`).
- **Action phase.** The fighting part of a round, at most `round_ticks` ticks. Ends early when the round is
  decided.
- **Planted phase.** After a plant: the fuse burns for `fuse_ticks` unless the charge is defused.
- **Round end.** A pause of `round_end_ticks` after a round is decided. Nothing moves or shoots.

**Round.** One buy phase, one action phase and, after a plant, a planted phase, ending with one team winning
it: attackers by elimination or detonation, defenders by elimination before a plant, by defusing, or on the
clock (`ROUND_WIN_*`). Rounds never draw.

**Half.** `rounds_per_half` rounds (6) with fixed sides. After the first half the teams swap sides and
credits reset to `start_credits`.

**Regulation.** The two halves. A game reaching `rounds_to_win` (7) rounds in regulation is won.

**Overtime pair.** When regulation ends level: two rounds, one with each team attacking, each starting with
`overtime_credits`. A team that takes both rounds of a pair wins the game. At most `overtime_max_pairs` (3)
pairs are played. Never call a single overtime round a "game".

**Tie-break.** What decides a game still level after the last overtime pair: rounds, then kills, then damage.
If all three are level the game is **drawn**. A game cut off by `max_ticks` is drawn whatever the score.

## Games and the platform

**Game** (also **match**). One map played to a result: first to `rounds_to_win` rounds, with overtime and
tie-break as above. A game is a pure function of (patch, map, bot A, bot B, seed). "Match" is the engine's word
(`ibc_engine::Match`, `ibc run`); "game" is the platform's (`games` in a series). They mean the same thing. A
game never means several maps.

**Series.** Several games between the same two submissions, one map each, distinct maps. The platform rates,
lists and requests series (`GET series/{id}`). A ranked series has `IBC_RANKED_GAMES` games (default 3; the
decided size is 3 to 5).

**Slot** (team slot). Which of the two seats a team takes in a game: **A** or **B** (`TEAM_A = 0`, `TEAM_B = 1`).
Team A attacks first. Unit ids `0..n-1` are team A and `n..2n-1` team B. The word "slot" also appears, always
qualified, as **unit slot** (`MatchInfo.unit_slot`, 0 to team size - 1, a unit's position within its team),
**weapon slot** (0 primary, 1 sidearm) and **ability slot** (0 signature, 1 bought).

**Side.** The role a team plays in a round: **attack** (`SIDE_ATTACK`, carries and plants the charge) or
**defend** (`SIDE_DEFEND`). A team's side changes at half-time and every overtime round; its slot never changes
within a game.

**Seed.** The 64-bit number that, with the patch, map and bots, fixes everything random in a game. Bots see its
low 32 bits as `MatchInfo.seed_lo`.

**Team.** In a game, the 3 to 5 units on one slot (the map sets the size). On the platform, a group of 1 to 4
people who share submissions and a rating, joined by a join code.

**Unit.** One body on the map, run by its own bot instance for the whole game.

**Bot.** The program a team writes. A **bot instance** is one copy of it running for one unit.

**Submission.** One uploaded, validated bot (a `wasm32-wasip1` module) of a team. Every submission is kept; the
archive is private to the team.

**Live submission.** The one submission per team that ranked games, other teams' scrims and tournaments use.
The team chooses it among its submissions; it cannot be unset once set.

**Ladder.** The ranking of all teams by rating (`GET ladder`).

**Rating.** A team's skill estimate, updated from ranked games. **Provisional** means the estimate is still too
uncertain to mean much (a new team or a fresh submission); clients show "provisional" instead of the number.

**Tier.** The label for a rating band: Iron, Bronze, Silver, Gold, Platinum, Diamond, Ascendant, Immortal,
Radiant. Display only; matchmaking ignores it.

**Ranked.** A series the platform schedules itself between live submissions, which changes ratings.

**Scrim** (unranked series). A series a team requests against another team, from a daily quota. It does not
change ratings. It uses the opponent's live submission, or exactly the submission they fielded in an earlier
series against you.

**Tournament.** A bracket (single or double elimination) run by the organisers on submissions locked at a
set time, seeded from the ladder, on maps hidden until it starts.

**Pairing.** One bracket match-up in a tournament round: two teams play every tournament map, one game each.
**decided_by** records how the pairing was won: `maps` (more maps won), `rounds` (maps level, more rounds won
across all its maps, overtime included), or `seed` (both level, the higher seed advanced; flagged for the
organisers). `null` while unfinished.

## Versions

**Engine version.** `version` in the root `Cargo.toml` (`ibc_engine::ENGINE_VERSION`). Names the mechanics.

**Ruleset hash.** 16 hex digits hashed from every number of the parsed `rules/*.toml` (`Ruleset::hash`). Changes
with any balance change, not with comments or formatting.

**Patch.** The identity a game is played under: engine version, ruleset hash, Wasmtime version and ABI version
(`ibc version --json`, `docs/release.md`). Two binaries with the same patch play every game identically. Do not
use "patch" for a code change in general.

**ABI version.** `ibc_abi::ABI_VERSION`: what bots can see and do (`docs/ABI.md`). Grows only by addition.

## Kits, equipment and abilities

**Kit.** What a unit drafts: base health, speed and exactly two ability slots (`T_KIT`, `KitRules`). Kit 0 is
played by a unit that never drafts. Kits are unique within a team, not across teams, and one unit drafts for the
whole team (`A_DRAFT_FOR`).

**Signature ability.** Ability slot 0 of a kit. Free: it is refilled to one charge at the start of every round.

**Bought ability.** Ability slot 1 of a kit. Its charges are bought in the buy phase (`ITEM_KIND_ABILITY_CHARGE`;
the charge costs the item's price plus the ability's).

**Ability charge.** One use of an ability. A unit holds at most one charge per ability (`max_charges` is 1 for
every ability); unused charges carry over to the next round.

**The charge** (objective). The bomb the attackers carry, plant on a site and the defenders defuse
(`DROP_CHARGE`, `S_HAS_CHARGE`). Not to be confused with an ability charge: say "the charge" for the bomb and
"ability charge" for the other.

**Plant / defuse / detonation.** Planting takes `plant_ticks` on a site; defusing takes `defuse_ticks`
(`defuse_kit_ticks` with a **defuse kit**). Detonation is the fuse running out, and wins the round for the
attackers.

**Credits.** Money, per unit, capped at `max_credits`, spent in the buy phase.

**Armour.** Points bought in the shop; absorbs `armor_absorb_pct` of each hit until used up.

## What a bot can know and spend

**Fuel.** The compute budget of a turn: WebAssembly instructions counted by Wasmtime plus a fixed charge per host
call (`FUEL_*`). Each unit gets `fuel_per_tick` per tick, times `fuel_first_tick_mult` on the draft tick and
`fuel_buy_mult` in the buy phase. Unused fuel is lost; a bot that runs out is paused and resumes next tick.

**Fog.** The host lists only what the caller may perceive: `T_ENEMIES`, `T_PROJECTILES`, `T_EFFECTS`, `T_DROPS`.
Teammates are always known. Vision and heard sounds are shared by the whole team. Events follow fog too: a team sees
an event only where it can see, except kills, plant, defuse, detonation and round end, which both teams always
get, without the position when it is not known (`POS_HIDDEN`).

**Vision.** A unit sees an enemy within `vision_range`, inside its cone (`facing ± fov_half`), with a clear
line; within `sense_radius` the cone does not matter.

**Sound.** What a unit hears (`T_SOUNDS`, `SOUND_*`): running footsteps, shots, plant and defuse starts,
abilities and decoys, within their radius and through walls, at a position snapped to the tile centre. Walking
(speed at most `walk_speed`) is silent.

**Team memory.** `MEM_TEAM`, `team_mem_bytes` (64) shared by a team, readable and writable by every teammate. A
teammate running later in the same tick sees a write. It is wiped at the start of every round and writable only in
action phases.

**Draft memory.** `MEM_DRAFT`, `draft_mem_bytes` (1024), written during the draft, read-only afterwards.

**Buy memory.** `MEM_BUY`, `buy_mem_bytes`, a memory scope like draft memory for the buy phase, smaller,
wiped every round and writable only in the buy phase.

**Unsteadiness.** A per-unit value that builds up with movement speed each tick and decays while the unit stands
still, like recoil. A weapon's startup (wind-up) grows with it, so moving costs time before a shot, never
accuracy.

**Brace.** The state after a completed startup in which a unit keeps firing at the weapon's normal rate. Moving,
a tick without firing, reloading, swapping weapon or starting an ability breaks it; turning does not.

**Recoil.** Heat added by each shot (`recoil_per_shot`), recovering on ticks without a shot. It widens the
pellet fan.

**Intent.** What `act` queues. It is validated at once but resolves with everyone else's when the tick is
stepped. One move, one facing and one primary action per tick.

## Keeping this file honest

Each entry was checked against the code on `main` when written. When a **(landing)** feature lands, drop the
marker and the "today" note in the same change. When a feature is dropped, remove its entry here and add a line
to `docs/DEPRECATED.md`. The generated pages (`docs/generated/`, `cargo xtask docs`) hold every number and id;
this file holds only words.
