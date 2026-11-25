# Kits and abilities

Each unit drafts a **kit** on tick 0 (`A_DRAFT`, a = kit id): base hit points, a speed percentage and two ability
slots. A unit that never drafts plays kit 0, the plain one. Every number here lives in `rules/abilities.toml`;
bots read the live values from `T_KIT` and `T_ABILITY` and should not hard-code them.

## Kits

| id | kit | slot 0 (free) | slot 1 (bought) | hp | speed |
|---|---|---|---|---|---|
| 0 | standard | none | none | 100 | 100% |
| 1 | lancer | dash | flash | 90 | 105% |
| 2 | shroud | decoy | smoke | 95 | 100% |
| 3 | seeker | recon | incendiary | 95 | 100% |
| 4 | bulwark | recon | barrier | 115 | 88% |
| 5 | mender | heal | smoke | 100 | 95% |
| 6 | kindler | dash | incendiary | 85 | 105% |

`hp` is what the unit starts every round with (and the ceiling for healing). `speed` scales the run speed, on top
of the weapon's `move_pct`.

## Abilities

Distances are in tiles (1024 units), times in ticks.

| id | ability | effect | price | max charges | cooldown | range | radius | duration | power | delay |
|---|---|---|---|---|---|---|---|---|---|---|
| 0 | smoke | `EFFECT_SMOKE` (1) | 300 | 2 | 4 | 10 | 1.75 | 14 | 0 | 1 |
| 1 | flash | `EFFECT_FLASH` (4) | 250 | 2 | 3 | 8 | 12 | 6 | 30 | 1 |
| 2 | incendiary | `EFFECT_FIRE` (2) | 350 | 1 | 6 | 8 | 1.5 | 8 | 12 | 1 |
| 3 | recon | `EFFECT_RECON` (5) | 0 | 1 | 0 | 12 | 6 | 5 | 3 | 1 |
| 4 | barrier | `EFFECT_BARRIER` (3) | 300 | 1 | 8 | 2.5 | 1.5 | 20 | 120 | 0 |
| 5 | dash | 8 (no constant yet) | 0 | 2 | 5 | 3 | 0 | 0 | 0 | 0 |
| 6 | decoy | `EFFECT_DECOY` (7) | 0 | 1 | 0 | 10 | 2 | 12 | 3 | 0 |
| 7 | heal | `EFFECT_HEAL` (6) | 0 | 2 | 6 | 4 | 1 | 6 | 7 | 0 |

## Charges and cooldowns

- An ability with `price` 0 is **free**: its charges are refilled to `max_charges` at the start of every round.
- Any other ability starts with no charges. Charges are **bought** in the buy phase with a shop item of kind
  `ITEM_KIND_ABILITY_CHARGE` whose `value` is the slot. Buying costs the item's price plus the ability's own
  `price`, and fails with `ERR_NOT_ALLOWED` at `max_charges`, `ERR_INSUFFICIENT_FUNDS` if the unit cannot pay
  both, `ERR_UNSUPPORTED` if the kit has nothing in that slot. Bought charges are kept until
  used, through deaths and the side swap.
- After a use the slot cannot be used again for `cooldown` ticks (`UnitInfo.cooldown` counts down to 0).
  Cooldowns are cleared at the start of a round.

## Using an ability

`A_ABILITY` with a = slot (0 or 1), b, c = target point. It is the tick's primary action, so it replaces a shot,
reload, plant or defuse queued the same tick, and it interrupts a plant or defuse channel. It does not interrupt
a wind-up or a reload already running. `act` validates against the current state and returns:

| code | when |
|---|---|
| `ERR_BAD_ARG` | slot is not 0 or 1, or the target point is outside the map |
| `ERR_UNSUPPORTED` | the kit has no ability in that slot |
| `ERR_NOT_READY` | no charges, or the slot is cooling down |
| `ERR_OUT_OF_RANGE` / `ERR_NOT_ALLOWED` | see the ability |

Abilities resolve in the intent step of the tick, in unit id order, before weapons and movement. If a use has
become impossible by then (a dash put a unit where a barrier was going), nothing happens and no charge is spent.
Every successful use is heard by enemies within `step_sound_radius` of the user as `SOUND_ABILITY`.

**Throwing.** Smoke, flash, incendiary, recon and decoy are devices thrown at the target point. They travel in
a straight line and land at the target, or `range` away if the target is further, or 64 units short of the
first wall or barrier in the way. Low cover does not stop them. A barrier's centre is placed by the same rule.

**Arming.** A device or barrier lies in the world as an effect (`T_EFFECTS`) from the tick it is used. It does
nothing for `delay` ticks, then it is active for `duration` ticks (a flash: for the one tick it pops).
`EffectInfo.ticks_left` counts both, so an effect is still arming while `ticks_left` is greater than its
`duration` (for a flash: greater than 1). With `delay` 1, enemies who can see where it landed get exactly one
turn to react. All effects are removed when a round starts.

`T_EFFECTS` lists your team's effects always, and an enemy effect when you can see its position (the cloud's
centre, a barrier's first end, the unit being healed).

## The abilities

### Smoke
A cloud of `radius` that blocks sight for `duration` ticks. Any line of sight that touches the circle is
blocked, in both directions and for both teams: units inside see nothing and are not seen, not even within
`sense_radius`. Bullets and thrown devices pass through; movement is not affected. A recon beacon and a flash
need a clear line, so smoke stops both. The cloud itself can be seen from outside (sight to the exact centre
point of a cloud ignores that cloud, unless a unit stands on that very point).
*Counters:* shoot through it, since sound still gives positions away; walk around; wait it out.

### Flash
Pops `delay` ticks after it is thrown. Every living unit, **friend or foe, the thrower included**, is blinded if
the pop is within `radius`, inside the unit's vision cone (`facing +/- fov_half`), and the line between them is
clear of walls, smoke and barriers. A unit facing it head-on is blinded for `duration` turns; this falls
linearly with the angle to `power` percent of that at the edge of the cone (rounded down, at least 1 turn).
A unit with the pop outside its cone is not affected at all, however close. A blinded unit (`S_BLINDED`,
`UnitInfo.blind` = turns left) sees only within `sense_radius`. A second flash does not add: the longer of the
two applies.
*Counters:* turn away. The device is visible while it arms and `turn_rate` is enough to put it outside the
cone in one tick; the price is a tick spent not looking at the fight. Smoke or a barrier in between also works.

### Incendiary
After `delay` ticks a circle of `radius` burns for `duration` ticks. Each tick, every living unit whose centre
is inside takes `power` damage through the normal damage path (armour absorbs its share), unless a wall or a
barrier lies between it and the centre. Fire burns **both teams**. Damage to enemies counts for the thrower's
statistics and kill reward; damage to teammates and to the thrower is not credited to anyone.
*Counters:* leave, or do not enter; stand behind a wall or barrier.

### Recon
A beacon that scans for `duration` ticks after its `delay`. Each tick, every enemy within `radius` that the
beacon has a clear line to (no wall, smoke or barrier) is marked `S_REVEALED` for `power` turns. A revealed
unit appears in `T_ENEMIES` of every unit of the other team, wherever they are and wherever they look.
*Counters:* break the line to the beacon: walls, smoke, a barrier. The marked unit sees `S_REVEALED` in its own
state and knows it is being watched.

### Barrier
A straight wall segment placed `range` in front of the user (nearer if the target point is nearer, and never
through a wall), at a right angle to the line from the user, reaching `radius` to each side or as far as the
nearest wall. `EffectInfo` holds its two ends in `(x, y)` and `(x2, y2)`. While it stands (`duration` ticks) it
blocks movement (a unit cannot come within `unit_radius` of the segment), sight, bullets and thrown devices, for
both teams. It has `power` hit points: every bullet it stops deals its normal damage to the barrier, and at 0
the barrier breaks. Bullets that arrive in the same sub-step as the breaking shot are still stopped.
Incendiary kills carry weapon id -1 in `EVENT_KILL`.
It cannot be placed touching any living unit (`ERR_NOT_ALLOWED`) or on the user's own position
(`ERR_OUT_OF_RANGE`). If the ruleset gives it a `delay`, it fails without effect when a unit is touching the
line at the moment it would arm.
*Counters:* shoot it down, walk around it, wait for it to expire.

### Dash
The unit moves instantly in a straight line towards the target point, `range` at most, and stops at the target
or at the last position where its circle is clear of walls, low cover and barriers. It does not slide. If it
cannot move at all the use fails with `ERR_OUT_OF_RANGE`. The dash happens before this tick's normal movement,
so both add up; a wind-up in progress keeps running.
*Counters:* it is loud (`SOUND_ABILITY` from where it started), and it ends in a straight line from there.

### Decoy
A device that plays fake sounds for `duration` ticks: every tick enemies within `step_sound_radius` of it hear a
`SOUND_STEP`, and every `power` ticks enemies within `shot_sound_radius` hear a `SOUND_SHOT`, both exactly as
real ones would arrive (same team, same snapped position).
*Counters:* an enemy within `radius` of the device hears `SOUND_DECOY` instead and knows. An enemy that sees the
device gets it in `T_EFFECTS`. And a real runner's footsteps move from tile to tile; a decoy's never do.

### Heal
Targets the living teammate nearest to the target point, the user itself included; there must be one within
`radius` of the point (`ERR_OUT_OF_RANGE` otherwise, also when that unit is further than `range` from the
user). Fails with `ERR_NOT_ALLOWED` if the target is at full health or a wall or barrier lies between the two.
The target then regains `power` hit points per tick for `duration` ticks, starting this tick, up to its kit's
`hp`. The effect follows the target and ends if it dies. Heals stack. Armour is not restored.
*Counters:* burst damage outpaces it; the heal is audible and is a tick the healer does not shoot.

## Timing, exactly

Effects tick once per action tick, after movement and bullets and before the objective is checked. For a use
on tick `T` with delay `d` and duration `n`:

- smoke and barrier block from the vision computed at the end of tick `T + d - 1` (so: from tick `T + d` as bots
  see it; a barrier with delay 0 already stops this tick's bullets and movement) for `n` ticks;
- fire burns, recon scans, the decoy plays and the heal mends on ticks `T + d` to `T + d + n - 1`;
- the flash pops on tick `T + d`, after that tick's turning, and units are blind for their next turns.
