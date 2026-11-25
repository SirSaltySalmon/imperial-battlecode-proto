# Vision feedback

Second review of [`vision.md`](vision.md), written 2026-11-25 against `main` after the vision was revised. It
replaces the first review, which was written against the older vision by a weaker model.

How far to trust this file:

- **Engine and rules claims were re-read in the code for this pass** (`crates/engine`, `crates/abi`,
  `rules/*.toml`, `docs/rules/combat.md`, `crates/rating/src/series.rs`, the replay record types).
- **Coordinator, CI and viewer claims are carried over from the first review and were only spot-checked.** They
  are grouped under "Carried over" at the end and agree with `docs/TODO.md`, but check them before relying on one.

Nothing here is implemented.

## 1. What the revised vision settled

| Topic | Decision in `vision.md` | Engine today | Work |
|---|---|---|---|
| Friendly fire | Every ability, never bullets | Same: flash and fire hit both teams, bullets pass teammates | None. Close the item in `docs/TODO.md`. New traps and the blast follow the ability rule |
| Rank labels | Iron … Diamond, Ascendant, Immortal, Radiant | No labels | Display only, on the ladder response. Do not matchmake inside a tier |
| Overtime | Capped, then tie-break, then a draw | 3 pairs, then rounds → kills → damage → **seed coin flip** | Replace the coin flip with a draw (section 3.6) |
| Spray | No randomness. Movement costs startup time. Sideways kick removed | Deterministic fan, shifted sideways by `extra / 2` | New firing model (section 3.1). The largest gameplay change in the vision |
| Vision | Shared by the team | Per unit (`Unit.seen`) | Core change in `world.rs` and `host.rs` (section 3.2) |
| Enemy information | Kits public, start-of-round credits public, equipment and charges hidden | Enemy kit only on a unit you see; credits and charges hidden | Additive ABI table (section 3.3) |
| Buying | Teammates can buy for each other | `A_BUY(item)` buys for yourself only | New action id (section 3.4) |
| Signature charges | One free per round, cap 1 | `price = 0` refills to `max_charges` | Data only, if my reading is right (section 3.5) |
| Replays | Every action and event; memory, logs and fuel for your own team only | One public file with both teams' memory writes; no logs, no per-turn fuel | Redaction and a hash split (section 3.8) |
| Rating | Per series, from map differential and rating gap | One Weng-Lin update per game | One batched update per series (section 3.9) |
| Scale | 200 to 800 teams | Never run above a handful | Section 3.11 |

## 2. Where the first review was wrong or is now stale

- **"7 games on 3 to 5 maps."** A misreading. The vision says each ranked game is 3 to 5 maps, each a full
  first-to-7 match. `plan_series` already picks that many distinct least-used maps, so this is
  `IBC_RANKED_GAMES = 3..5` and no code. The "two-step plan with replacement" it proposed is not needed.
- **"Add a field on `[[ability]]` for signature charges."** Not needed with a cap of 1 (section 3.5).
- **"Prefer keeping per-game Weng-Lin."** The vision now decides the opposite. The underlying worry (do not
  double-count) is still right and section 3.9 keeps it.
- **"One overtime pair."** The vision moved back to the current cap of 3.
- **Emerald and the Ascendant/Immortal order.** Fixed in the vision.
- **"Random spray cannot exist in a deterministic public ruleset."** Correct, and the vision dropped it. What the
  first review missed is the consequence for one-pellet weapons (section 3.1).
- **"Tournament shape contradicts itself."** I read "all teams play each other on all maps" as "each bracket
  pairing plays every tournament map", which does not contradict elimination. Confirm that reading (section 4).
- Its "Already in the game" list held up against the code, with one gap: it did not say that vision is per unit.

## 3. What each decision costs

### 3.1 Firing: startup instead of kick

Today (`combat/firing.rs`, `combat/state.rs`): every pull has a wind-up of `windup` ticks; at release the fan
opens by `move_spread * speed + recoil_spread * recoil` and the whole fan is pushed `extra / 2` to alternating
sides. The vision is right that the push is worthless as a penalty: a bot can compute it and aim the other way.

Three things the new model has to answer before code:

1. **Spread means nothing on a one-pellet weapon once the kick is gone.** Spread is the angle between adjacent
   pellets. The pistol and marksman have `pellets = 1`, so with no shift they are exact at any speed and any
   recoil. "Spread grows as you keep shooting" will do nothing for them. In a deterministic game the only honest
   penalties are fan geometry (multi-pellet weapons lose their outer bullets) and time. So single-shot weapons
   must be balanced with startup, cooldown, magazine and falloff. Do not reintroduce a computed offset; it is the
   kick again.
2. **"Stopped by the end of startup" and "spread grows with walking speed" cannot both read the release tick.**
   If the unit is standing when bullets leave, release-tick speed is always 0. Either spread reads the speed at
   the trigger pull, or `move_spread` goes away and movement is paid for in startup alone. I recommend the second:
   one penalty, one number per weapon, and one change to measure.
3. **A bot can stop for one tick and then pull.** If startup depends only on the speed at the pull, the penalty
   is one tick whatever the weapon. To make it bite, track unsteadiness as heat, exactly like recoil: it rises
   with speed each tick, decays per tick standing, and startup is `windup + f(heat)`. That is one general
   mechanism rather than a special case, it is visible state a bot can plan around, and it must be hashed and
   exposed on `UnitInfo` (additive).

The second line of the vision ("after startup, normal fire rate; stop or move and start again") is a braced
state. Consequences worth knowing first:

- Pistol and rifle are `windup = 1, cooldown = 0`, so a pull takes 2 ticks today and would take 1 when braced.
  Sustained damage doubles for them. Every time-to-kill table in `docs/rules/combat.md` moves.
- Firing roots the unit. Strafing while shooting disappears, and the SMG loses its run-and-gun role unless its
  movement penalty is near zero. Make the penalty a per-weapon number so that role survives.
- Define what breaks the brace: a move intent (yes), a tick with no `A_FIRE` while ready (the vision says yes),
  a reload (decide), turning (I would say no, `turn_rate` is already the limit), a swap (yes, it already cancels).
- A move during startup should cancel the startup, not slide the unit to a halt. A forced deceleration is a
  second movement path.
- `vx`/`vy` report commanded velocity even when a wall blocked the move (`docs/TODO.md`). Fix that first, or the
  heat will charge units for moves that did not happen.

All of this is inside `crates/engine/src/combat` behind the existing seams (`start_fire`, `pre_step`), plus
weapon fields and one `UnitInfo` field.

### 3.2 Shared vision

`World::compute_vision` fills a per-unit `seen` mask and `host::rows` filters `T_ENEMIES`, `T_PROJECTILES`,
`T_EFFECTS` and `T_DROPS` per unit. Shared vision is the union over living teammates. Compute it once per team
per tick; that is cheaper than today, not dearer.

- This closes the `docs/TODO.md` decision "whether teammates' sightings should be shared automatically".
- **Sounds are not mentioned.** `Unit.sounds` is per unit. If vision is shared and sound is not, bots will relay
  sounds through team memory, which is the busywork shared vision removes. I would share both. Decide.
- Aim is still clamped to the shooter's own cone, but a unit can now fire at an enemy only a teammate sees, for
  example through smoke. That raises the value of any spotter and lowers the value of recon, which already
  reveals to everyone. Expect to re-tune recon.
- Team memory stops being a sightings channel and becomes a plan channel, so 64 bytes is more plausible than the
  first review thought.
- `host.rs` and `world.rs` are core files, and what `T_ENEMIES` returns changes for existing bots. Layout stays,
  meaning widens. Ship it alone and re-record goldens.

### 3.3 What you know about the enemy

Needed: kits of all five enemies without line of sight, and their credits as of the start of the buy phase.
`T_ENEMIES` cannot carry this (it lists living units in sight). Add one table at the end of the ABI, readable in
every phase, with a row per enemy: unit id, kit, credits at round start. Snapshot the credits in `start_round`
and hash the snapshot.

Check the wording "not the equipment they held". `UnitInfo` shows `weapon`, `armor` and `hp` of any enemy in
sight today. I read the vision as "no global loadout view", and would keep weapon in hand visible on sight;
hiding it would make an enemy with a marksman look like one with a pistol until it fires. Armour is the
debatable one.

"You can see your team's equipment at all times" is not fully true today: `UnitInfo` shows only the weapon in
hand, not the second one (`docs/TODO.md`).

### 3.4 Buying for teammates, the director, and "different kits"

These three are one problem.

- `A_BUY` takes `a = item` and applies to the caller. A target argument cannot reuse `b`, because existing bots
  pass 0 there and 0 is a valid unit slot. Add `A_BUY_FOR` at the end of the action ids.
- Credits are per unit. The buyer pays. Note the side effect: the trade-in refund (`refund_pct = 100`) means A
  buys B a rifle, B swaps it for an SMG and keeps the difference. That is a credit transfer. It is harmless if
  transfers are meant to be possible; say so in the rules rather than discovering it.
- **Do not add a director instance.** Each unit already has its own wasm instance for the whole match, and
  turn order inside a team is deterministic. With buy-for and a shared buy memory, a team that wants a director
  makes slot 0 the director. A sixth instance type is a new entry point, a new fuel budget and a new fault rule
  for no added capability.
- "Pick different kits" reads as unique per team. The draft is one simultaneous tick, so two units that pick the
  same kit collide with no tick left to repick, and the loser falls to kit 0. Either the draft gets several
  ticks, or one unit drafts for the team (`A_DRAFT_FOR`). Six real kits for five seats also leaves almost no
  choice; uniqueness wants eight or more.
- Kits have exactly two ability slots and `UnitInfo.charges` is `[i32; 2]`. "Many more abilities" therefore
  means more kits. If a kit should ever hold three or four, decide before the ABI is published, because that
  field cannot grow.

### 3.5 Signature charges

"One free charge every round, unused charges kept, cap 1" is, at cap 1, the same as "refill to 1 every round".
That is exactly what `price = 0, max_charges = 1` does now. The change is data: dash and heal go from 2 to 1,
and smoke and flash from 2 to 1 if the cap covers bought abilities too. "Can buy new charges" has no effect on
a signature at cap 1; it only applies to slot 1.

If "cap 1" meant "one purchase per round" rather than "hold one", this section is wrong and the first review's
extra field is needed. Confirm.

At most two casts per unit per round means each cast has to matter. That fits the stated goal, and it means
ability power numbers will need to go up, measured one at a time.

### 3.6 Draws and overtime

- `economy::tiebreak` ends with `(w.seed & 1)`. Replace it with a draw. `World::finish` takes a winner `i32`;
  it needs a no-winner outcome. The replay and job formats already carry `"draw"` and the rater scores it as a
  half.
- "3 overtime games" is ambiguous: overtime is played in pairs, one round per side, and the cap is 3 pairs. An
  odd number of rounds would give one team an extra attack. Keep pairs.
- `max_ticks` also ends a match through `tiebreak`. Decide whether running out of ticks is a draw too.
- A bracket cannot seat a draw (section 3.10).

### 3.7 Detonation blast

In `World::step_objective`, on fuse expiry, kill living units within `blast_radius` of the charge before
`end_round`. Death already drops weapons and strips the loadout next round, so "lose your gun" needs no new
rule. It hits both teams, which matches the friendly-fire decision.

- Pick the radius from run speed and fuse. A unit runs 700 units per tick (about 0.68 tiles), the fuse is 40
  ticks, defuse is 7. A radius of 8 tiles is roughly 12 ticks of running, so a defender must give up the defuse
  with 12 ticks left or die. That is the choice the vision wants. Check it against site sizes with
  `ibc-maptool analyze`.
- Blast kills have no attacker. Say whether they count in the kill tie-break (I would say no).
- The economy example in `docs/rules/economy.md` is pinned by a test and assumes detonation kills nobody.
- This only matters once rounds end by detonation. The last measurement was 0 of 390.

### 3.8 Replays that hide the other team

Today a replay is the list of accepted inputs per tick plus a state hash, and inputs include both teams'
`MemWrite`. Logs and per-turn fuel are not recorded anywhere; the summary has fuel totals only.

- A redacted replay drops the opponent's `MemWrite` inputs. Re-simulation still produces the same world, but
  memory is part of `state_hash`, so verification fails. Split the hash into a world hash and a per-team memory
  hash, and store both per tick. Then a redacted file verifies on the world hash.
- Keep one full replay server-side as the record. Serve a per-team view with a token, and a both-redacted view
  publicly. The current public endpoint serves the full file with no token.
- Logs and fuel per turn are new recording work. They are not simulation inputs and must stay out of the hash.
  This is the same shape as the missing debug drawings in `docs/TODO.md`; do them together.
- Honest limit: every action is still public, so behaviour can be imitated. What is protected is intent (memory
  and logs), which is what the vision asks for.
- A worker sees both modules. Privacy assumes workers are trusted.

### 3.9 Rating per series

Each game is rated on arrival today (`db/ratings.rs`). I read "do not adjust on per-map performance" as "the
round score inside a map does not matter, the count of maps won does".

Recommended form: when the series finishes, compute each map's Weng-Lin delta from the **pre-series** ratings and
apply the sum once. It uses map differential and rating gap, counts each map once, does not depend on the order
maps finished in, and a 3-0 moves more than a 2-1. A drawn map is a half. The ledger event becomes one per
series, or stays one per game with the prior pinned; the second is the smaller change.

An odd map count gives one team an extra game in slot A. Slot only affects turn order within a tick, and sides
already swap at half-time, so this is small. Measure slot bias once with a mirror before worrying about it.

### 3.10 Weekly tournament

Still entirely unbuilt: pause flag, submission snapshot, bracket table, hidden map list. New points from the
revised vision:

- At 800 teams, single elimination is 799 series and double is about 1,600. That is small next to the ladder.
- A series over N maps can end level (even N, or a drawn map). Use an odd N and a fixed tie-break: round
  differential across the series, then the higher seed advances.
- Hidden maps become ranked maps after the hour while the bracket is still running on them. That is fine since
  tournament bots are locked, but lint and measure a map before it is revealed, not after.
- A balance patch during a bracket kills it today, because a patch change voids running jobs.

### 3.11 Scale

- Light bots: about 26 ms per match measured (`docs/TODO.md`). A 4-map series is about 0.1 core-seconds; pairing
  all 800 teams once is about 40 core-seconds. Not a constraint.
- Worst case is unmeasured. A match can grant 10 units × 100,000 fuel × up to 3,000 ticks, about 3 × 10⁹ fuel,
  which is plausibly seconds to tens of seconds of wasm. A full ladder round would then be core-hours. Measure a
  bot that burns its whole budget before choosing a ladder cadence, and lower the governor's 10 core-second
  prior to what you find.
- Storage: every upload kept, times 800 teams, with no cap on stored submissions and no upload rate limit.
- "Queue unranked against any team's bot, including old versions" at this size is unlimited black-box access to
  every archived bot of every team, bounded only by 100 games a day. Decide whether "old versions" means yours
  or anyone's.

## 4. Decisions taken on 2026-11-25, and what they imply

These are now written into `vision.md`. Where section 3 offers alternatives, this section wins.

| Decision | Consequence |
|---|---|
| Movement adds no spread. Startup comes from an unsteadiness value that builds with speed and decays standing | `move_spread` is removed from `rules/weapons.toml` and `WeaponRules`, not left at zero. New per-weapon numbers for gain, decay and startup per point. New hashed unit state, shown on `UnitInfo` |
| Sounds are shared with vision | `T_SOUNDS` becomes a team table. Same patch as shared vision |
| A unit holds at most one charge per ability | `max_charges = 1` on every ability. Smoke, flash, dash and heal drop from 2. Data only |
| Kits unique within a team, not across teams. One unit drafts for the team | `A_DRAFT_FOR` at the end of the action ids, a sibling of `A_BUY_FOR` and not merged with it. Turn order within a team is deterministic, so a single drafter cannot collide with itself. A kit already taken by a teammate is refused with `ERR_NOT_ALLOWED`. The draft can stay one tick |
| Enemy in sight: weapon visible, HP and armour hidden | `hp` and `armor` read 0 for enemies, as `credits` already does. **Not enough on its own.** `T_EVENTS` is unfiltered (`host.rs` returns every row of `World.events` to every unit) and `EVENT_DAMAGE` carries attacker, target, position and amount. Any bot can rebuild enemy HP from it, and it also gives away the position of every unit that takes damage, through fog. Events need the same team-sight filter as the other tables, and damage dealt to an enemy needs its amount hidden or coarsened. This is a fog leak today, independent of the HP decision |
| Overtime is 3 pairs; out of ticks is a draw | Matches current rules except the coin flip. Both paths go through `economy::tiebreak` |
| Plant 12, defuse 21, kit defuse 16, fuse 80 | See the tick budget below |
| Two abilities per agent: one free signature, one bought | This is the kit shape the engine already has (slot 0 free, slot 1 bought), so `UnitInfo.charges: [i32; 2]` is safe to freeze. "Many more abilities" means more kits |
| Reload and swap break the brace; turning does not | A reload mid-fight costs the reload plus a fresh startup. Magazine size becomes a larger balance lever than it is today |
| Blast radius 12 tiles, through walls | See below |
| Events follow fog; kills always visible without position; damage shows the amount only | See below |
| Tournaments: single and double elimination, every pairing plays every map, started by hand | No scheduler work. The pause, snapshot and bracket builder are an admin action with a team list, a format and a map list as inputs |
| Team memory is wiped at the end of every round | Zero it in `start_round` next to buy memory. No shared channel carries anything across rounds except draft memory, which is read-only. Cross-round opponent modelling lives in each unit's private wasm memory. With shared vision every unit sees the same thing, so that is workable |
| Buy phase is about 6 ticks | `buy_ticks = 6`. Buy fuel is 4x, so buy-phase cost doubles |
| Tournament field: everyone, top N, or a hand-picked list | The bracket builder takes a team list as input. The three options are three ways to produce the list |
| Archive is private. Other teams' old bots are reachable only as "the version I already played" | The request names a past series, not a submission id, and the server resolves the pinned submission. Check what the public series listing exposes: it selects each side's submission hash, which lets anyone count the distinct versions a team has fielded |

**Tick budget.** A round can now last buy 6 + action 120 + fuse 80 + pause 2 = 208 ticks, up from 165. With 12
regulation rounds and 6 overtime rounds that is 3,744 ticks, over `max_ticks = 3000`. Raise `max_ticks` to
about 4,000 in the same patch, or long matches will end as draws by the clock. Worst-case fuel per match rises
by about a third. The longer match is accepted.

**Timing shape.** Plant 12 against an action timer of 120 means a plant must start by tick 108. Defuse 21
against fuse 80 leaves 59 ticks to retake, and a defuse cannot start after fuse tick 59. A kit defuse of 16
moves that to 64. Tripling
plant and defuse while doubling the fuse are three changes; land them one at a time with a bench run between.

**Blast radius 12 tiles, through walls.** A plain distance test from the charge, in `step_objective`, with
`blast_radius = 12288` in `rules/core.toml`. At run speed (about 0.68 tiles per tick) getting clear from the
charge takes about 18 ticks. So a defender must be running by fuse tick 62, a full defuse must start by tick
59 and a kit defuse by tick 64. A kit holder who starts at the last moment and is interrupted cannot escape.

On `crossfire` (32 by 17 tiles) a 12-tile radius is a circle 24 tiles across: it still spans the full height
of the map but leaves room at the ends. Add a lint to `ibc-maptool`: from every plantable tile, a tile outside
the blast must be reachable. Without it a small map can make a lost round also cost every defender's gun.

**Events and fog.** Today `host.rs` hands every row of `World.events` to every unit, so this is a real change:

- Filter `T_EVENTS` per team with the shared-vision test on the event position. It belongs in the shared-vision
  patch, since it uses the same team mask.
- `EVENT_KILL` is always delivered, with killer and victim. When the position is not visible to the team,
  deliver it with the position blanked. Use a sentinel the docs name, not `(0, 0)`, which is a real point.
- `EVENT_DAMAGE` keeps its amount. Hiding `hp` and `armor` on enemy `UnitInfo` then gives exactly the stated
  effect: a bot can sum the damage it saw and guess, but armour absorbs half of each bullet until it runs out,
  so the split is unknown without knowing what was bought.
- "Tab shows who is alive" is the enemy roster table from section 3.3 with an alive flag per unit.
  `MatchInfo.enemies_alive` already gives the count.
- Sounds stay separate from events. A shot heard through a wall is still a sound, not a damage event.
- The viewer and the replay are unaffected: fog is applied when a bot reads the table, not when it is recorded.

**Tournament pairings that end level.** Decided: seed from the ladder, aim for an odd map count, then total
rounds won across the pairing's maps, then the higher seed advances and the result is flagged. Implications:

- An odd map count does not rule out a level pairing, because a map can now be drawn (1-1 with one draw). The
  rounds tie-break is therefore needed, not a fallback for a mistake.
- Store how each pairing was decided (`maps`, `rounds`, `seed`) on the bracket edge. The flag is that field
  being `seed`; an organiser view filters on it.
- Count regulation and overtime rounds alike, and say so, since overtime rounds are played in pairs and inflate
  totals evenly.
- The seeding snapshot must be taken at the lock, with the bots, so the last tie-break cannot move mid-bracket.

Nothing in `vision.md` is open any more.

## 5. Suggested order

1. ~~Answer the open questions.~~ Done on 2026-11-25; section 4 records the answers.
2. **Bot pool first.** The reference bots do not draft, buy for value, or use abilities. Until they do, no change
   below can be measured, and a league table before and after is required for each.
3. Re-run `ibc-bench` on the current pool for a baseline.
4. Data-only patches, one at a time: charge caps, ability delays, plant, defuse and fuse lengths.
5. Draw instead of coin flip. Small, isolated, touches `economy::tiebreak` and `World::finish`.
6. Shared vision (and sounds). Core change, alone, goldens re-recorded.
7. The firing model. Fix reported velocity first. Regenerate the combat tables.
8. One additive ABI patch: enemy roster table, round log table, buy memory scope, `A_BUY_FOR`, `A_DRAFT_FOR`,
   unsteadiness and second weapon on `UnitInfo`.
9. Detonation blast, once rounds actually reach detonation.
10. Thrown projectiles on the bullet step, then traps.
11. In parallel on `crates/coordinator`, since none of it changes a match: series size and the batched rating
    update, replay redaction, turning the ranked scheduler on, upload and API limits, then the tournament runner.
12. In parallel on `viewer/`: rules from the replay header, debug drawings, buy menu, then human play.

Steps 4 to 10 each change the game bots are written against. Each is its own engine version.

## 6. Carried over from the first review (spot-checked only)

- **Already in the game:** first to 7 with two halves of 6; round stops when decided, with a 2-tick pause;
  facing rate limit; running is audible and walking is not; smoke, flash, fire, recon, barrier, dash, decoy and
  heal; barriers are segment effects in `T_EFFECTS` and so already distinguishable from map walls; draft memory
  1024 bytes, team memory 64; dead units drop weapons and restart on the sidearm; fogged snapshots and a fuel
  meter the bot cannot set.
- **Memory phases:** add `MEM_BUY` at the end of the scope ids, store and hash it on `World`, zero it in
  `start_round`, gate team-memory writes to action phases. A buy-phase team-memory write that works today will
  return `ERR_NOT_ALLOWED`.
- **Match history for bots:** `T_EVENTS` is last tick only. Add a round-log table from `World.rounds`; do not
  widen `T_EVENTS`.
- **Thrown abilities** appear at the target instantly today. A slow throwable should be a second user of the
  bullet step and show in `T_PROJECTILES`, not a travel loop per ability. Traps are a new effect id.
- **Platform:** the ranked scheduler is off by default (`IBC_RANKED_INTERVAL_SECS = 0`); the live slot cannot be
  emptied, which already blocks rating freezes; language fuel multipliers are self-declared; no account ban, no
  upload or API rate limits, no web UI; deployment files have never been run.
- **Development experience:** a glossary (game, match, series, slot, patch) is the cheapest high-value doc;
  generate rules and ABI tables from `rules/*.toml` and `crates/abi` rather than hand-keeping them; keep CI path
  filters but do not skip in-crate tests; the native sim has no fuel limit, so report balance from the wasm
  backend; the viewer hardcodes core rules and cannot yet send intents, so human play starts with rules in the
  replay header.
