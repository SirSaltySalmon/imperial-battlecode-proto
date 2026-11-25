# Combat (v0)

How guns work, exactly. `docs/RULES.md` has the overview; this page has the formulas the engine uses
(`crates/engine/src/combat/`), the weapon values and what they add up to. All numbers come from
`rules/weapons.toml`, which bots read at runtime through `T_WEAPON`. Nothing in gunplay is random.

Units: 1024 per tile, 65536 angle units per turn, time in ticks. All arithmetic is integer; `/` rounds
towards zero.

The tables between `generated` markers are written by a test from the ruleset and from duels played in the
engine. After changing `rules/weapons.toml` refresh them with

```
IBC_BLESS=1 cargo test -p ibc-engine --test combat_ttk
```

`cargo test -p ibc-engine` fails while they are stale.

Watch the same duels, and the other combat tests, in the replay viewer. Record them, then open the combat lab
(`viewer/lab.html`), which draws with the viewer's theme and units:

```
cargo run -p ibc-combatviz -- --out viewer/public/lab.json
```

From `viewer/`, `npm run dev` and open `/lab.html`. The page also accepts a dropped `lab.json`. Pass `--out` to
write the recording somewhere else (the default is `out/combat-lab/lab.json`).

## One trigger pull

`A_FIRE(angle)` on tick `T` with a weapon of wind-up `w` and cooldown `c`:

| Tick | What happens |
|---|---|
| `T` | The wind-up starts. Refused with `ERR_NOT_READY` if there is no weapon in hand, the magazine is empty, or a wind-up, reload or cooldown is running. If no facing was queued, the unit turns towards the aim. |
| `T` .. `T+w-1` | Winding up. Enemies who see the unit get `S_WINDUP` and the aim angle; `UnitInfo.channel` counts down. Recoil recovers on these ticks. |
| `T+w` | The bullets leave, before anyone moves on that tick. One round is used, recoil rises by `recoil_per_shot`, the shot is heard. |
| `T+w+1` .. `T+w+c` | Cooldown: a new wind-up cannot start. |
| `T+w+1+c` | The next `A_FIRE` is accepted. |

So one pull takes `windup + 1 + cooldown` ticks, and a target that sees the wind-up start has `windup - 1`
whole ticks of movement before the bullets leave, plus the flight time.

## Where the bullets go

At the release tick, with `speed` the length of the unit's velocity on that tick (0 when it queued no move),
`recoil` its recoil heat and `shots` the number of pulls it has released so far this round:

```
extra  = min(move_spread * speed / 1024 + recoil_spread * recoil, 32768)
S      = spread + extra                          angle between adjacent pellets
shift  = extra / 2        if shots is even
       = -(extra / 2)     if shots is odd
centre = facing + clamp(aim - facing, -fov_half, +fov_half) + shift
pellet i (0 .. pellets-1) flies along   centre + (2*i - (pellets - 1)) * S / 2
```

- The fan is symmetric about `centre`. An odd pellet count puts one bullet on the centre line; an even count
  straddles it by `S / 2`. The full fan is `(pellets - 1) * S` wide.
- For an even pellet count the inner pellet on the side away from the shift stays `spread / 2` off the
  aim line whatever `extra` is; the rest of the fan swings away from it. An odd fan has no such pellet.
- A settled, standing shooter has `extra = 0`: the fan is centred exactly on the aim and `spread` apart.
- Moving and recoil do two things at once: they open the fan and they push all of it off the aim line, to
  alternating sides. A single-pellet weapon misses by `extra / 2`; it hits a target at distance `d` only
  while `extra / 2` is less than the angle the target covers, about `unit_radius * 10430 / d`.
- `aim - facing` is the signed shortest difference. You can only shoot inside your view cone; the facing used
  is the one on the release tick, so keep turning during the wind-up.
- Spread uses the speed of the release tick only. Stopping for that one tick gives the standing fan.

Each bullet starts on the rim of the shooter's circle (`unit_radius` from its centre along its direction).
If a wall or a bullet-blocking effect lies between the centre and that point the bullet is spent at once.

## Flight and damage

Bullets fly at `bullet_speed` per tick in `substeps` slices, together with unit movement. In each slice a
bullet is swept along its path: it stops at the first wall, and hits the first enemy circle it enters if that
comes before the wall. All hits of a slice are applied together, so two units can kill each other. There is
no friendly fire. A bullet that has flown `range` is spent: a weapon reaches exactly `range` from the muzzle
in every direction, and a hit past that does nothing.

Damage of one bullet that hits after flying `t`:

```
t <= falloff_start:   damage
otherwise:            damage * (100 - (100 - falloff_min_pct) * min(t - falloff_start, range - falloff_start)
                                       / (range - falloff_start)) / 100
```

Armour then absorbs `armor_absorb_pct` percent of each bullet until it is used up (`rules/economy.toml`).

## Recoil

- Each release adds `recoil_per_shot` to the unit's recoil heat (`UnitInfo.recoil`, visible to everyone).
- On every tick on which the unit does not release a shot, heat drops by the `recoil_recovery` of the weapon
  in hand, down to zero. Wind-up, cooldown and reload ticks all count.
- Heat belongs to the unit, not the weapon: swapping does not clear it. It resets at the start of a round.
- Holding the trigger therefore gains `recoil_per_shot - recoil_recovery * (windup + cooldown)` per pull.
  Where that is positive, sustained fire walks off the target; pause and it settles.

## Magazines, reload, cancel

- `magazine` is trigger pulls, not bullets. Reserve ammunition is unlimited.
- `A_RELOAD` on tick `T` fills the magazine `reload_ticks` ticks later: the first `A_FIRE` accepted is on
  tick `T + reload_ticks`. It is ignored when the magazine is full or a reload is already running (it does
  not restart). The weapon cannot fire while reloading.
- `A_RELOAD` during a wind-up abandons the shot (the round stays in the magazine) and starts the reload.
- `A_CANCEL` stops a wind-up or a reload. A cancelled wind-up costs nothing: no round, no recoil, no
  cooldown, so it can be used as a feint. A cancelled reload leaves the magazine as it was.
- Starting a plant or defuse cancels both, as does dying. A unit killed during its wind-up does not fire;
  bullets already in the air keep flying.
- `A_SWAP(slot)` abandons the wind-up or reload of the weapon put away. The swap is the tick's action, so
  the new weapon can fire from the next tick; a cooldown still running is kept, so swapping never lets you
  shoot sooner. Swapping to the slot already in hand does nothing.
- Holding a weapon scales top speed by `move_pct`; with empty hands there is no penalty.

## Weapons on the ground

- A unit that dies drops its weapons where it died, except the default sidearm everyone is handed each
  round, as `T_DROPS` rows of kind `DROP_WEAPON` with `item` = weapon id.
- `A_DROP(DROP_WEAPON)` puts the weapon in hand at the unit's feet; the unit switches to its other weapon if
  it has one. With empty hands it does nothing.
- `A_PICKUP` takes the nearest drop within `pickup_reach`. A weapon goes into its own slot; a weapon already
  there is put down in its place, so picking up swaps. The unit keeps using the slot it had in hand, unless
  its hands were empty.
- **A weapon on the ground holds no ammunition.** A weapon picked up has an empty magazine and must be
  reloaded. Dropping and picking up is therefore never a faster reload, and handing a teammate a weapon
  costs them one reload.
- Dropping, or picking up into the slot in hand, cancels wind-up and reload, like a swap.
- The displaced weapon lies at your feet, so it is the nearest drop for your next `A_PICKUP`: repeating the
  action swaps back and forth. Step away to reach something else.
- Weapons are never created during a round, and only a dead unit's default sidearm disappears. The ground is cleared when the next round starts;
  survivors keep what they hold.

## The weapons

Values, straight from `rules/weapons.toml`:

<!-- generated:weapons -->
| field | pistol | rifle | smg | shotgun | marksman | lmg |
|---|---:|---:|---:|---:|---:|---:|
| `id` | 0 | 1 | 2 | 3 | 4 | 5 |
| `slot` | sidearm | primary | primary | primary | primary | primary |
| `price` | 0 | 2700 | 1500 | 1800 | 3200 | 2300 |
| `damage` | 26 | 22 | 17 | 15 | 75 | 16 |
| `pellets` | 1 | 3 | 2 | 7 | 1 | 5 |
| `spread` | 0 | 250 | 200 | 260 | 0 | 300 |
| `move_spread` | 600 | 1200 | 150 | 300 | 2400 | 2000 |
| `recoil_spread` | 120 | 250 | 100 | 60 | 300 | 60 |
| `recoil_per_shot` | 2 | 3 | 2 | 4 | 4 | 2 |
| `recoil_recovery` | 1 | 1 | 1 | 2 | 1 | 1 |
| `windup` | 1 | 1 | 1 | 1 | 2 | 2 |
| `cooldown` | 0 | 0 | 0 | 2 | 2 | 0 |
| `magazine` | 12 | 8 | 15 | 5 | 5 | 20 |
| `reload_ticks` | 3 | 4 | 3 | 5 | 5 | 7 |
| `bullet_speed` | 8192 | 14336 | 9216 | 8192 | 20480 | 12288 |
| `range` | 20480 | 30720 | 16384 | 9216 | 40960 | 28672 |
| `falloff_start` | 8192 | 16384 | 6144 | 3072 | 40960 | 14336 |
| `falloff_min_pct` | 50 | 70 | 40 | 25 | 100 | 60 |
| `move_pct` | 100 | 90 | 100 | 95 | 80 | 70 |
| `kill_reward` | 300 | 200 | 400 | 500 | 100 | 150 |
<!-- /generated:weapons -->

What they add up to (fan width in angle units for a settled, standing shooter; top speed in units per tick
with the standard kit; magazine time is holding the trigger from full):

<!-- generated:derived -->
| weapon | ticks per pull | damage per pull (close / at range) | full fan width | top speed | range (tiles) | bullet (tiles/tick) | ticks to empty + reload |
|---|---:|---:|---:|---:|---:|---:|---:|
| pistol | 2 | 26 / 13 | 0 | 700 | 20.0 | 8.0 | 24 + 3 |
| rifle | 2 | 66 / 45 | 500 | 630 | 30.0 | 14.0 | 16 + 4 |
| smg | 2 | 34 / 12 | 200 | 700 | 16.0 | 9.0 | 30 + 3 |
| shotgun | 4 | 105 / 21 | 1560 | 665 | 9.0 | 8.0 | 20 + 5 |
| marksman | 5 | 75 / 75 | 0 | 560 | 40.0 | 20.0 | 25 + 5 |
| lmg | 3 | 80 / 45 | 1200 | 490 | 28.0 | 12.0 | 60 + 7 |
<!-- /generated:derived -->

Roles:

- **pistol**: the free sidearm. Accurate single shots, fine on the move, falls off quickly.
- **smg**: cheap run-and-gun. Hardly spreads when moving and carries at full speed; short reach.
- **shotgun**: seven pellets in a wide fan. One pull kills an unarmoured unit inside about three tiles and
  the fan is hard to sidestep; nearly harmless past six tiles. Slow pump, small magazine.
- **rifle**: the all-rounder. A tight three-bullet burst that kills in two pulls while all three land and
  still reaches across the map, if you stand still and pace your fire.
- **marksman**: one fast bullet, no falloff, two hits kill anything. Slow to cycle, and a two-tick wind-up
  that the target can see. Cannot hit while moving.
- **lmg**: heavy and slow. Five bullets a pull, a magazine that lasts sixty ticks and no recoil build-up, for
  holding a lane; slow to carry, to start and above all to reload.

## Time to kill

Measured by `crates/engine/tests/combat_ttk.rs` in the engine, not computed: ticks from the first `A_FIRE`
until a stationary target dies. The shooter fires whenever the weapon is ready and its recoil shift after
the wind-up is no larger than the angle the target covers, and reloads when empty. "Running" means it also
strafes across the line of fire at the weapon's top speed (turning round every four ticks) and aims from
where it will be when the bullets leave. "no kill" means the target was still alive after 100 ticks.

Target with 100 health and no armour:

<!-- generated:ttk -->
| weapon | 5 tiles standing | 5 tiles running | 10 tiles standing | 10 tiles running | 20 tiles standing | 20 tiles running |
|---|---:|---:|---:|---:|---:|---:|
| pistol | 8 | 8 | 11 | no kill | 23 | no kill |
| rifle | 4 | no kill | 8 | no kill | 18 | no kill |
| smg | 6 | 6 | 11 | 16 | no kill | no kill |
| shotgun | 6 | 10 | no kill | no kill | no kill | no kill |
| marksman | 8 | no kill | 8 | no kill | 8 | no kill |
| lmg | 6 | 21 | 9 | no kill | 25 | no kill |
<!-- /generated:ttk -->

Target with 100 health and the heaviest armour in the shop:

<!-- generated:ttk-armor -->
| weapon | 5 tiles standing | 5 tiles running | 10 tiles standing | 10 tiles running | 20 tiles standing | 20 tiles running |
|---|---:|---:|---:|---:|---:|---:|
| pistol | 12 | 12 | 16 | no kill | 35 | no kill |
| rifle | 10 | no kill | 16 | no kill | 30 | no kill |
| smg | 10 | 10 | 21 | 27 | no kill | no kill |
| shotgun | 10 | 18 | no kill | no kill | no kill | no kill |
| marksman | 8 | no kill | 8 | no kill | 8 | no kill |
| lmg | 6 | 30 | 12 | no kill | 37 | no kill |
<!-- /generated:ttk-armor -->

These are duels against a target that stands still. A target that strafes at range dodges any single shot
aimed where it stood (the bullet takes time to arrive), so real fights take longer and reward leading.
