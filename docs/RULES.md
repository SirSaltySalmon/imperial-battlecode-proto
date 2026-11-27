# Game rules (v0)

A deterministic 2D attackers-vs-defenders tactical shooter played by bots. Every unit runs its own bot instance.
Every number below lives in `rules/*.toml`; bots read the live values at runtime (`T_RULES_CORE`, `T_WEAPON`, ...),
so never hard-code them. This file describes the mechanics; the TOML files are the source of truth for values.

## World

- Continuous 2D. Positions are fixed-point: 1024 units per tile, origin top-left, +x right, +y down.
- Angles are 0..65535 per full turn. 0 points along +x and angles grow towards +y (clockwise on screen).
- All distances are Euclidean. There is no grid movement.
- The map is a tile grid: floor, wall (blocks movement, sight and bullets), low cover (blocks movement only).
  Units are circles of `unit_radius`. Plant sites are tile regions. The static map is known to everyone.
- Teams of 3 to 5, set by the map. Unit ids `0..n-1` are team A, `n..2n-1` team B.

## Match flow

- Tick 0 is the **draft**: each unit may pick a kit (`A_DRAFT`), or pick one for a teammate (`A_DRAFT_FOR`), and
  write draft memory. It has extra fuel. Kits are unique within a team (kit 0, the plain one, excepted).
- Then rounds. Each round: **buy phase** (`buy_ticks`, units stand at spawn, `A_BUY` applies immediately), **action
  phase** (`round_ticks`), and after a plant the **planted phase** (`fuse_ticks`).
- Attackers win a round by eliminating the defenders or by detonation. Defenders win by eliminating the attackers
  before a plant, by defusing, or when the action timer runs out. After a plant, killing all attackers is not
  enough: the charge must be defused.
- Team A attacks for the first `rounds_per_half` rounds, then sides swap and credits reset. First to
  `rounds_to_win` wins. If regulation ends level, overtime is played in pairs of rounds, one per side; see
  [rules/economy.md](rules/economy.md). Details of weapons and abilities are in [rules/combat.md](rules/combat.md)
  and [rules/abilities.md](rules/abilities.md).
- If overtime does not separate the teams, the tie-break does (rounds, then kills, then damage); if that is
  level too the match is a **draw**. A match that reaches `max_ticks` is a draw whatever the score.

## A tick

All living units' bots run against the same start-of-tick snapshot and queue intents. Nobody acts first. Then:

1. **Intents.** Facing turns towards the requested angle by at most `turn_rate`. Primary actions start
   (fire, reload, plant, defuse, ability, pickup, drop, swap). Velocity is set from `A_MOVE`.
2. **Weapons.** Ability uses whose startup ends happen. A move cancels a weapon startup and breaks the brace;
   timers tick; weapon startups that finish release their bullets.
3. **Movement and bullets**, in `substeps` slices: units move with collision (they slide along walls), bullets move
   and are swept against walls and unit circles. All hits within a slice are applied together, so units can trade.
   Thrown ability devices fly on the same step, over units, and land at the first wall or barrier or at their range.
   Afterwards each unit's velocity is set to the distance it actually covered (what bots read as `vx`/`vy`) and its
   unsteadiness is updated from it.
4. **Effects and statuses** tick.
5. **Objective.** Plant and defuse channels progress, the fuse burns, round-end conditions are checked.
6. **Sound and vision** for the next tick are computed.

A unit may queue one move, one facing and one primary action per tick; a later call of the same kind replaces
the earlier one.

## Vision and sound

- A unit sees an enemy when it is within `vision_range`, inside the cone `facing +/- fov_half`, and the straight
  line between them is not blocked. Within `sense_radius` it senses in every direction (still needs line of sight).
- **Vision is shared by the team.** A team sees the union of what its living units see, and every unit reads
  that same picture. You can therefore fire at an enemy only a teammate sees, but your shot is still clamped to
  your own cone and stopped by walls. Teammates' positions and state are always known.
- Enemies in sight show their position, movement, state flags and the weapon in hand. Their health, armour,
  credits, ammunition and ability charges are hidden.
- Running (speed above `walk_speed`) makes footsteps that enemies within `step_sound_radius` hear. Shots are heard
  within `shot_sound_radius`. Plant and defuse starts are audible. Heard positions are snapped to the tile centre.
  **Hearing is shared too:** a sound reaches the whole team when any living unit of it is in range.
- **Events follow the fog.** Damage is reported only where the team sees it (or to the unit's own team). Kills
  are always reported, with the position hidden when the team did not see it. Plant, defuse, detonation and
  round end are always reported. Fog is applied to what bots read; replays record everything.

## What a team knows

- `T_ROSTER`: every unit of both teams with its kit, its credits as the round's buy phase began (a snapshot,
  so enemy spending is not visible), and whether it is alive. Readable in every phase. Equipment is not in it.
- `T_ROUNDS`: every finished round, with the winner, how it ended (`ROUND_WIN_*`), the attacking team, its length
  and the score after it.
- Your own and your teammates' `UnitInfo` include the weapon in the other slot (`other_weapon`) and the ticks
  before the weapon in hand may fire again (`fire_cooldown`). Enemy rows read 0 there.

## Shooting

- `A_FIRE(angle)` starts a startup of `windup + unsteadiness * startup_per_point / 100` ticks (halves round up).
  Enemies who see you get the `S_WINDUP` flag and your aim angle, so they have that long to react. You can only
  fire within your cone: the shot is clamped to `facing +/- fov_half`. If you queue no facing, firing turns you
  towards the aim.
- **Unsteadiness** (`UnitInfo.unsteadiness`, visible to everyone) is heat like recoil: every tick it drops by the
  weapon's `unsteady_decay` and rises by `unsteady_gain` per 1024 units/tick you actually moved, up to
  `unsteady_max`. Walking holds it low, running drives it to the cap, and stopping for one tick before a pull
  does not erase a run.
- **Firing roots you.** A move cancels a startup, and a pull on a tick you also move is refused
  (`ERR_NOT_ALLOWED`). When the startup ends the bullets leave and you are **braced** (`S_BRACED`): while you
  stand and pull on every tick the weapon is ready, each pull leaves on the tick it is made, one every
  `1 + cooldown` ticks. A move, a ready tick without `A_FIRE`, a reload, a swap, a cancel, a plant or a defuse
  breaks the brace; turning does not.
- The weapon releases `pellets` bullets centred on the aim, `S = spread + recoil_spread * recoil` apart.
  Movement adds no spread and nothing pushes the fan sideways: a single-pellet weapon is exact, and sustained
  fire only opens multi-pellet fans. Nothing is random: you can compute exactly where your bullets go, but not
  where the target will be.
- Bullets are fast objects, not hitscan: they fly at `bullet_speed` per tick, are stopped by walls, and hit the
  first enemy circle on their path. Damage is full up to `falloff_start` and drops linearly to `falloff_min_pct`
  at `range`. Armour absorbs a share of each hit until it is used up. There is no friendly fire.
- At long range sustained fire with a multi-pellet weapon loses its outer pellets (an even fan, all of them):
  read `UnitInfo.recoil` and pause to let it settle, at the price of a fresh startup.
- Each shot adds `recoil_per_shot`; recoil recovers by `recoil_recovery` on ticks without a shot. Magazines hold
  `magazine` trigger pulls; reloading takes `reload_ticks`. Holding a weapon scales your top speed by `move_pct`.

## Objective

- One attacker starts each round with the charge. It drops where the carrier dies; any attacker can `A_PICKUP` it.
- `A_PLANT` works for the carrier standing on a site and takes `plant_ticks`. `A_DEFUSE` works for a defender
  within `interact_reach` of the planted charge and takes `defuse_ticks` (`defuse_kit_ticks` with a kit).
  Moving, firing or `A_CANCEL` interrupts either channel.
- "Standing on a site" means the unit's centre is on a tile of that site; a site can have any shape. Bots see
  each site as a bounding box (`SiteInfo`), whose centre need not be on the site, and a **plant point**
  (`SiteInfo.plant_x`, `plant_y`): the centre of the site tile nearest the box centre, ties going to the first
  tile in row-major order (top row first, then leftmost). A carrier standing on the plant point is on the site.
- **Detonation blast.** When the fuse runs out, every living unit within `blast_radius` (12 tiles) of the charge
  dies, attackers and defenders alike. It is a plain distance: walls do not shield anyone. The attackers win
  the round even if the blast kills them. Blast deaths are ordinary deaths (weapons drop, the loadout is lost)
  with no killer: `EVENT_KILL` has actor -1 and weapon -1, nobody is paid for them and they do not count as
  kills in the tie-break. At run speed clearing the radius from the charge takes about 18 ticks.

## Economy

- Everyone starts each half with `start_credits`. Round win and loss pay out, with a bonus for consecutive losses.
  Kills pay the weapon's `kill_reward`; a plant pays the whole attacking team.
- The shop (`T_ITEM`) sells weapons, armour, ability charges and the defuse kit during the buy phase. A unit can
  buy for a teammate (`A_BUY_FOR`) and pays for it. Trade-ins go to whoever replaces the item, so credits can
  move between teammates; that is allowed. Details in [rules/economy.md](rules/economy.md).
- Survivors keep their weapons and armour into the next round; everyone gets a sidearm.

## Kits and abilities

Each unit drafts a kit: base health, speed and two ability slots. Abilities (smoke, flash, incendiary, recon,
barrier, dash, decoy, heal, trap, sonar, concussion) are defined in `rules/abilities.toml`, which ships a plain
kit 0 and nine kits with abilities (`docs/rules/abilities.md`).

## Compute and memory

- Each unit gets `fuel_per_tick` fuel per tick (times a per-language multiplier), more on the draft tick and in
  the buy phase. Fuel is counted WebAssembly instructions plus a fixed charge per host call (`FUEL_*`).
  `memory.copy` and `memory.fill` cost fuel per byte, so large `memcpy`/`memset` calls are expensive.
- A bot that runs out is paused where it is and continues on its next tick with a fresh budget. Unused fuel is lost.
- Memory is shared by the team and readable in every phase; each scope is writable in one phase only. Units run
  in an order that alternates teams and rotates each tick; a teammate that runs later in the same tick sees
  your write.
  - Team memory (`MEM_TEAM`): `team_mem_bytes`, **zeroed at the start of every round** (nothing carries from one
    round to the next through it), writable in the action phases only.
  - Draft memory (`MEM_DRAFT`): `draft_mem_bytes`, written during the draft, read-only afterwards.
  - Buy memory (`MEM_BUY`): `buy_mem_bytes`, zeroed at every round start, writable in the buy phase only.
  Writing outside a scope's phase returns `ERR_NOT_ALLOWED`.
- Units of a dead bot (crash or exit) idle for the rest of the match.
