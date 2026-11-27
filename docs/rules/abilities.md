# Kits and abilities

Each unit drafts a **kit** on tick 0 (`A_DRAFT`, a = kit id): base hit points, a speed percentage and two ability
slots. A unit that never drafts plays kit 0, the plain one. Kits are unique within a team: drafting a kit a
teammate already holds is refused with `ERR_NOT_ALLOWED` (kit 0 excepted; the other team's picks do not count).
A draft applies at once, and teammates run in a fixed order, so one unit can draft for the whole team with
`A_DRAFT_FOR` (a = kit id, b = the teammate's slot) without collisions. Every number here lives in `rules/abilities.toml`;
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
| 7 | trapper | trap | concussion | 100 | 95% |
| 8 | echo | sonar | incendiary | 95 | 100% |
| 9 | breaker | decoy | concussion | 95 | 105% |

`hp` is what the unit starts every round with (and the ceiling for healing). `speed` scales the run speed, on top
of the weapon's `move_pct`.

## Abilities

Distances are in tiles (1024 units), times in ticks.

| id | ability | effect | price | max charges | cooldown | range | radius | duration | power | startup (`delay`) | speed |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 0 | smoke | `EFFECT_SMOKE` (1) | 300 | 1 | 4 | 10 | 1.75 | 14 | 0 | 1 | 3 |
| 1 | flash | `EFFECT_FLASH` (4) | 250 | 1 | 3 | 8 | 12 | 6 | 30 | 1 | 4 |
| 2 | incendiary | `EFFECT_FIRE` (2) | 350 | 1 | 6 | 8 | 1.5 | 8 | 12 | 1 | 2.5 |
| 3 | recon | `EFFECT_RECON` (5) | 0 | 1 | 0 | 12 | 6 | 5 | 3 | 1 | 0 |
| 4 | barrier | `EFFECT_BARRIER` (3) | 300 | 1 | 8 | 2.5 | 1.5 | 20 | 120 | 1 | 0 |
| 5 | dash | `EFFECT_DASH` (8) | 0 | 1 | 5 | 3 | 0 | 0 | 0 | 0 | 0 |
| 6 | decoy | `EFFECT_DECOY` (7) | 0 | 1 | 0 | 10 | 2 | 12 | 3 | 1 | 0 |
| 7 | heal | `EFFECT_HEAL` (6) | 0 | 1 | 6 | 4 | 1 | 6 | 7 | 1 | 0 |
| 8 | trap | `EFFECT_TRAP` (9) | 0 | 1 | 0 | 3 | 0.75 | 300 | 4 | 3 | 0 |
| 9 | sonar | `EFFECT_SONAR` (10) | 0 | 1 | 0 | 8 | 5 | 0 | 3 | 2 | 0 |
| 10 | concussion | `EFFECT_CONCUSSION` (11) | 250 | 1 | 4 | 8 | 2 | 0 | 4 | 1 | 0 |

`speed` is in tiles per tick; 0 means the device is not thrown but appears where it lands at once. The slowest
bullet flies 8 tiles per tick.

## Charges and cooldowns

- A unit holds at most one charge of each ability (`max_charges` = 1 throughout the shipped ruleset), so a unit
  casts at most twice a round: its free signature and one bought charge.
- An ability with `price` 0 is **free**: its charges are refilled to `max_charges` at the start of every round.
- Any other ability starts with no charges. Charges are **bought** in the buy phase with a shop item of kind
  `ITEM_KIND_ABILITY_CHARGE` whose `value` is the slot. Buying costs the item's price plus the ability's own
  `price`, and fails with `ERR_NOT_ALLOWED` at `max_charges`, `ERR_INSUFFICIENT_FUNDS` if the unit cannot pay
  both, `ERR_UNSUPPORTED` if the kit has nothing in that slot. Bought charges are kept until
  used, through deaths and the side swap.
- After an ability happens the slot cannot be used again for `cooldown` ticks (`UnitInfo.cooldown` counts down
  to 0). Cooldowns are cleared at the start of a round.

## Using an ability

`A_ABILITY` with a = slot (0 or 1), b, c = target point. It is the tick's primary action, so it replaces a shot,
reload, plant or defuse queued the same tick, and it interrupts a plant or defuse channel. It does not interrupt
a wind-up or a reload already running. `act` validates against the current state and returns:

| code | when |
|---|---|
| `ERR_BAD_ARG` | slot is not 0 or 1, or the target point is outside the map |
| `ERR_UNSUPPORTED` | the kit has no ability in that slot |
| `ERR_NOT_READY` | no charges, the slot is cooling down, or a use is already in its startup |
| `ERR_OUT_OF_RANGE` / `ERR_NOT_ALLOWED` | see the ability |

Abilities resolve in the intent step of the tick, in unit id order, before weapons and movement. If a use has
become impossible by then (a dash put a unit where a barrier was going), nothing happens and no charge is spent.

**Startup.** Most abilities have a startup: `AbilityRules.delay` ticks between the use and the ability
happening. A use on tick `T` with startup `d` happens on tick `T + d`, after that tick's intents and before
weapons and movement: exactly where a use queued on `T + d` without a startup would happen. With `d` = 0 it
happens in the intent step of tick `T`. During the `d` turns in between (ticks `T + 1` to `T + d`):

- the unit has `S_CASTING` in its state, which enemies who see it can read too;
- it can move and turn, but its hands are busy: `act` refuses every other primary action (fire, reload, swap,
  plant, defuse, pickup, drop, another ability) with `ERR_NOT_READY`;
- `A_CANCEL` abandons the use, and so does dying. Nothing is spent;
- taking damage does not interrupt it.

When the startup ends the use is checked again from where the unit then stands (charges, range, a barrier not
touching anyone, a heal target still hurt and in reach); if it fails nothing happens and nothing is spent. When it
happens the charge is spent, the cooldown starts and enemies within `step_sound_radius` of the user hear
`SOUND_ABILITY`.

**Throwing.** Smoke, flash, incendiary, recon, decoy, trap, sonar and concussion are devices aimed at the target point: at the target,
or `range` away if the target is further. A device whose ability has `speed` > 0 (smoke, flash, incendiary) is
thrown: it leaves the thrower's centre as a projectile and flies `speed` units per tick in a straight line on
the same sub-stepped step as bullets. It flies over units, friend or foe, and through smoke; low cover does not
stop it. It lands where its flight ends, or 64 units short of the first wall or barrier it meets, and its effect
appears there in that sub-step and acts from that tick. In flight it is a row of `T_PROJECTILES` with
`kind` = ability id + 1 and `weapon` = -1, visible to the thrower's team and to enemies who can see it. A
device with `speed` 0 (recon, decoy, trap, sonar, concussion) appears at its landing point at once, which follows the same rule
measured along the whole line. A barrier's centre is placed by that rule too.

**Effects.** A device or barrier lies in the world as an effect (`T_EFFECTS`) from the moment it lands and is
active at once for `duration` ticks (a flash, a sonar pulse and a concussion: for the one tick they pop, so a thrown flash pops in the tick it
lands and never shows in `T_EFFECTS`; watch `T_PROJECTILES`). `EffectInfo.ticks_left` counts down the ticks
it has left. All effects and devices in flight are removed when a round starts.

`T_EFFECTS` lists your team's effects always, and an enemy effect when you can see its position (the cloud's
centre, a barrier's first end, the unit being healed). An enemy **trap** is listed only when you can see it and
it is within `sense_radius` of you: further away it is hidden even in plain view.

**Being hit.** Every time an effect gives a unit a status (a flash blinds it, a concussion or a trap slows it, a
trap, a sonar pulse or a recon beacon reveals it), that unit's team gets an `EVENT_EFFECT` with the effect kind
in `value`, and so does the other team if it sees the unit. Fire reports its damage as `EVENT_DAMAGE` instead.
Only the unit hit reads where it came from: the angle to the effect's centre and the centre itself (`dir`,
`src_x`, `src_y`), never the thrower's position. The thrower is named (`actor`) only to its own team. A recon
beacon reports each tick it marks the unit, fire each tick it burns. See `docs/ABI.md`.

## The abilities

### Smoke
A cloud of `radius` that blocks sight for `duration` ticks. Any line of sight that touches the circle is
blocked, in both directions and for both teams: units inside see nothing and are not seen, not even within
`sense_radius`. Bullets and thrown devices pass through; movement is not affected. A recon beacon and a flash
need a clear line, so smoke stops both. The cloud itself can be seen from outside (sight to the exact centre
point of a cloud ignores that cloud, unless a unit stands on that very point).
*Counters:* shoot through it, since sound still gives positions away; walk around; wait it out.

### Flash
Pops in the tick it lands. Every living unit, **friend or foe, the thrower included**, is blinded if
the pop is within `radius`, inside the unit's vision cone (`facing +/- fov_half`), and the line between them is
clear of walls, smoke and barriers. A unit facing it head-on is blinded for `duration` turns; this falls
linearly with the angle to `power` percent of that at the edge of the cone (rounded down, at least 1 turn).
A unit with the pop outside its cone is not affected at all, however close. A blinded unit (`S_BLINDED`,
`UnitInfo.blind` = turns left) sees only within `sense_radius`. A second flash does not add: the longer of the
two applies. Each unit blinded gets an `EVENT_EFFECT` (`value` = `EFFECT_FLASH`) that tells it alone where the
flash popped, not who threw it (see **Being hit** above).
*Counters:* turn away. The thrower is visible casting (`S_CASTING`) and the device in flight
(`T_PROJECTILES`), and `turn_rate` is enough to put it outside the cone in one tick; the price is a tick spent
not looking at the fight. Smoke or a barrier in between also works.

### Incendiary
From the tick it lands a circle of `radius` burns for `duration` ticks. Each tick, every living unit whose centre
is inside takes `power` damage through the normal damage path (armour absorbs its share), unless a wall or a
barrier lies between it and the centre. Fire burns **both teams**. Damage to enemies counts for the thrower's
statistics and kill reward; damage to teammates and to the thrower is not credited to anyone.
*Counters:* leave, or do not enter; stand behind a wall or barrier.

### Recon
A beacon that scans for `duration` ticks. Each tick, every enemy within `radius` that the
beacon has a clear line to (no wall, smoke or barrier) is marked `S_REVEALED` for `power` turns. A revealed
unit appears in `T_ENEMIES` of every unit of the other team, wherever they are and wherever they look.
*Counters:* break the line to the beacon: walls, smoke, a barrier. The marked unit sees `S_REVEALED` in its own
state and an `EVENT_EFFECT` pointing at the beacon, so it knows it is being watched and from where.

### Barrier
A straight wall segment placed `range` in front of the user (nearer if the target point is nearer, and never
through a wall), at a right angle to the line from the user, reaching `radius` to each side or as far as the
nearest wall. `EffectInfo` holds its two ends in `(x, y)` and `(x2, y2)`. While it stands (`duration` ticks) it
blocks movement (a unit cannot come within `unit_radius` of the segment), sight, bullets and thrown devices, for
both teams. It has `power` hit points: every bullet it stops deals its normal damage to the barrier, and at 0
the barrier breaks. Bullets that arrive in the same sub-step as the breaking shot are still stopped.
Incendiary kills carry weapon id -1 in `EVENT_KILL`.
It cannot be placed touching any living unit (`ERR_NOT_ALLOWED`) or on the user's own position
(`ERR_OUT_OF_RANGE`); this is checked again when the startup ends, so a unit that steps into the line during
it makes the use fail (no charge is spent).
*Counters:* shoot it down, walk around it, wait for it to expire.

### Dash
The unit moves instantly in a straight line towards the target point, `range` at most, and stops at the target
or at the last position where its circle is clear of walls, low cover and barriers. It does not slide. If it
cannot move at all the use fails with `ERR_OUT_OF_RANGE`. It has no startup: the dash happens before this tick's
normal movement, so both add up; a wind-up in progress keeps running.
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
The target then regains `power` hit points per tick for `duration` ticks, starting the tick the heal happens,
up to its kit's `hp`. The effect follows the target and ends if it dies. Heals stack. Armour is not restored.
*Counters:* burst damage outpaces it; the heal is audible and is a tick the healer does not shoot.

### Trap
A hidden trigger placed like a device that is not thrown, at most `range` away, when its startup (`delay`) ends.
It then lies armed for `duration` ticks. On every tick it is armed, it springs on **every living unit, friend or foe, the user
included**, whose centre came within `radius` of it during that tick: the test is swept along the straight line
from where the unit stood when effects last ticked to where it stands now, so running or dashing across it
counts. A unit standing on it when it appears is caught too. Each caught unit is slowed (`S_SLOWED`, half speed) and
revealed (`S_REVEALED`) for `power` turns, and the trap is spent. Enemies get it in `T_EFFECTS` only within
`sense_radius` (see above); the owner's team always sees it.
*Counters:* walk, and look: at walking speed a unit gets about two turns between seeing a trap and reaching it.
Bullets do not touch it; it is cleared when the round ends.

### Sonar
A device that pulses once, where it lands when its startup (`delay`) ends. Every living unit within `radius` of it, **friend
or foe, through walls, smoke and barriers**, is marked `S_REVEALED` for `power` turns, so the user's teammates
standing in the pulse are shown to the enemy just as the enemies are shown to them.
*Counters:* the user is `S_CASTING` for the startup; leave where it could land, or use the turns it shows you to
reposition.

### Concussion
A device that pops once, where it lands when its startup (`delay`) ends. Every living unit within `radius` that no wall or
barrier shelters from the centre (smoke does not), **both teams and the user included**, is slowed for `power`
turns and loses what it was doing: a plant or defuse channel (all progress), a weapon startup and its brace (no
shot, no ammunition used), a reload (the magazine keeps what it had) and an ability use in its startup (nothing
spent).
*Counters:* stand behind cover from where it lands; a defuser who sees the user casting can step off and
restart after the pop.

## Timing, exactly

Effects tick once per action tick, after movement and bullets and before the objective is checked. For a use
on tick `T` with startup `d` and duration `n`, an ability that is not thrown happens on tick `T + d` before
movement, and then:

- smoke and barrier block from the vision computed at the end of tick `T + d` (so: from tick `T + d + 1` as bots
  see it) for `n` ticks; a barrier already stops the bullets and movement of tick `T + d`;
- fire burns, recon scans, the decoy plays and the heal mends on ticks `T + d` to `T + d + n - 1`;
- a trap lies armed on ticks `T + d` to `T + d + n - 1` and tests the movement of each of those ticks;
- the flash, the sonar pulse and the concussion pop on tick `T + d`, after that tick's turning and movement, and
  the statuses they give last for the unit's next turns.

A thrown device leaves the thrower on tick `T + d` before movement and lands in tick `T + d + k - 1`, where `k`
is `ceil(distance / speed)` (fewer if a wall or barrier is in the way); read `T + d + k - 1` for `T + d` above.
The SDK's `tactics::ticks_to_effect` computes this offset. The slot is ready again `cooldown + 1` ticks after the
tick the ability happens in.
