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

## Startup and the brace

A unit that is not braced pulls with `A_FIRE(angle)` on tick `T`. Its startup is

```
s = windup + (unsteadiness * startup_per_point + 50) / 100
```

ticks, with `unsteadiness` the unit's heat on that tick (below). With cooldown `c`:

| Tick | What happens |
|---|---|
| `T` | The startup begins. Refused with `ERR_NOT_READY` if there is no weapon in hand, the magazine is empty, or a startup, reload or cooldown is running, and with `ERR_NOT_ALLOWED` if the unit also queued a move this tick. If no facing was queued, the unit turns towards the aim. |
| `T` .. `T+s-1` | Starting up. Enemies who see the unit get `S_WINDUP` and the aim angle; `UnitInfo.channel` counts down. Recoil recovers on these ticks. A move intent on any of them cancels the startup: no round, no recoil, no cooldown. |
| `T+s` | The bullets leave, before anyone moves on that tick. One round is used, recoil rises by `recoil_per_shot`, the shot is heard. The unit is now **braced** (`S_BRACED`). |
| `T+s+1` .. `T+s+c` | Cooldown: no pull is accepted. Skipping `A_FIRE` here does not matter. |
| `T+s+1+c` | The weapon is ready. A braced unit's pull leaves **on this tick**, and the cooldown starts again. |

So a braced unit fires once every `1 + cooldown` ticks for as long as it stands and keeps pulling. The brace
breaks on:

- a move intent (any speed above 0), whether or not the unit also pulls, and any tick on which the unit is
  displaced at all (a dash);
- a tick on which the weapon is ready and the unit queued no `A_FIRE` (a different primary action counts as
  not pulling);
- `A_RELOAD`, `A_SWAP`, `A_CANCEL`, dropping or picking up into the hand, starting a plant or defuse, dying.

Turning does not break it: `turn_rate` is already the limit. A braced unit with an empty magazine stays braced
(the weapon is not ready) until it reloads, which breaks it. After the brace breaks the next pull starts up
again.

Startup and braced ticks are therefore always standing ticks: by the time the bullets leave the unit has
stopped. A target that sees the startup begin has `s - 1` whole ticks of movement before the bullets leave,
plus the flight time; braced shots give no warning.

## Unsteadiness

Every unit has unsteadiness heat (`UnitInfo.unsteadiness`, visible to everyone). After each tick's movement,
with `moved` the distance the unit actually covered that tick (what bots then read as `vx`/`vy`; a move into a
wall covers nothing) and the numbers of the weapon in hand:

```
settled      = max(unsteadiness - unsteady_decay, 0)
unsteadiness = min(settled + unsteady_gain * moved / 1024, max(unsteady_max, settled))
```

- A pace that adds no more than `unsteady_decay` a tick holds the heat at what one tick adds; a faster pace
  climbs to `unsteady_max`. With the current numbers walking costs at most one tick of startup and a run costs
  the most.
- It decays by `unsteady_decay` a tick, so stopping for one tick before a pull removes only a little of a run.
- It belongs to the unit and uses the numbers of the weapon in hand. `unsteady_max` only caps what moving
  adds: heat brought from a weapon with a higher cap just decays, so swapping never sheds it. With empty
  hands it stays as it is. It resets at the start of a round.

Unsteadiness only lengthens the startup. Movement adds no spread and never moves the bullets.

## Where the bullets go

When the bullets leave, with `recoil` the unit's recoil heat:

```
extra  = min(recoil_spread * recoil, 32768)
S      = spread + extra                          angle between adjacent pellets
centre = facing + clamp(aim - facing, -fov_half, +fov_half)
pellet i (0 .. pellets-1) flies along   centre + (2*i - (pellets - 1)) * S / 2
```

- The fan is symmetric about the aim. An odd pellet count puts one bullet on the aim line; an even count
  straddles it by `S / 2`. The full fan is `(pellets - 1) * S` wide.
- A settled shooter has `extra = 0`: pellets are `spread` apart.
- Recoil only opens the fan. A single-pellet weapon (pistol, marksman) is exact at any heat, so those weapons
  are balanced by startup, cooldown, magazine and falloff. A multi-pellet weapon held on target loses its outer
  pellets first; an even fan has no pellet on the aim and misses entirely once `S / 2` exceeds the angle the
  target covers, about `unit_radius * 10430 / d` at distance `d`.
- `aim - facing` is the signed shortest difference. You can only shoot inside your view cone; the facing used
  is the one on the release tick, so keep turning during the startup.

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

Armour then absorbs `armor_absorb_pct` percent of each bullet until it is used up (`rules/economy.toml`). At 100
it is plain extra health, taken point for point before hp: with no armour penetration, light armour (25) means 125
damage to kill and heavy (50) means 150.

## Recoil

- Each release adds `recoil_per_shot` to the unit's recoil heat (`UnitInfo.recoil`, visible to everyone).
- On every tick on which the unit does not release a shot, heat drops by the `recoil_recovery` of the weapon
  in hand, down to zero. Startup, cooldown and reload ticks all count.
- Heat belongs to the unit, not the weapon: swapping does not clear it. It resets at the start of a round.
- Braced, holding the trigger therefore gains `recoil_per_shot - recoil_recovery * cooldown` per pull. Where
  that is positive, a multi-pellet fan keeps opening; pause and it settles, at the price of a fresh startup.

## Magazines, reload, cancel

- `magazine` is trigger pulls, not bullets. Reserve ammunition is unlimited.
- `A_RELOAD` on tick `T` fills the magazine `reload_ticks` ticks later: the first `A_FIRE` accepted is on
  tick `T + reload_ticks`. It is ignored when the magazine is full or a reload is already running (it does
  not restart). The weapon cannot fire while reloading.
- `A_RELOAD` during a startup abandons the shot (the round stays in the magazine) and starts the reload.
- `A_CANCEL` stops a startup or a reload and breaks the brace. A cancelled startup costs nothing: no round,
  no recoil, no cooldown, so it can be used as a feint. A cancelled reload leaves the magazine as it was.
- Starting a plant or defuse cancels both, as does dying. A unit killed during its startup does not fire;
  bullets already in the air keep flying.
- `A_SWAP(slot)` abandons the startup or reload of the weapon put away. The swap is the tick's action, so
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
- Dropping, or picking up into the slot in hand, cancels startup and reload, like a swap.
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
| `damage` | 26 | 22 | 17 | 15 | 74 | 16 |
| `pellets` | 1 | 3 | 2 | 7 | 1 | 5 |
| `spread` | 0 | 250 | 200 | 260 | 0 | 300 |
| `recoil_spread` | 120 | 250 | 100 | 60 | 300 | 60 |
| `recoil_per_shot` | 2 | 3 | 2 | 4 | 4 | 2 |
| `recoil_recovery` | 1 | 1 | 1 | 2 | 1 | 1 |
| `windup` | 1 | 1 | 1 | 1 | 2 | 2 |
| `cooldown` | 1 | 1 | 0 | 3 | 4 | 0 |
| `magazine` | 12 | 8 | 15 | 5 | 5 | 20 |
| `reload_ticks` | 3 | 4 | 3 | 5 | 5 | 7 |
| `bullet_speed` | 8192 | 14336 | 9216 | 8192 | 20480 | 12288 |
| `range` | 10240 | 30720 | 16384 | 9216 | 40960 | 28672 |
| `falloff_start` | 4096 | 16384 | 6144 | 3072 | 24576 | 14336 |
| `falloff_min_pct` | 50 | 70 | 40 | 25 | 86 | 60 |
| `move_pct` | 100 | 90 | 100 | 95 | 80 | 70 |
| `kill_reward` | 300 | 200 | 400 | 500 | 100 | 150 |
| `unsteady_gain` | 4 | 8 | 2 | 6 | 12 | 12 |
| `unsteady_decay` | 2 | 2 | 2 | 2 | 2 | 2 |
| `unsteady_max` | 4 | 12 | 4 | 8 | 16 | 16 |
| `startup_per_point` | 25 | 25 | 25 | 25 | 25 | 25 |
<!-- /generated:weapons -->

What they add up to (startup in ticks for a steady unit, one that has walked long enough to settle, and one
that has run long enough to reach the cap; fan width in angle units for a settled shooter; top speed in units
per tick with the standard kit; magazine time is holding the trigger braced from full):

<!-- generated:derived -->
| weapon | startup (steady / walked / ran) | ticks per braced shot | damage per pull (close / at range) | full fan width | top speed | range (tiles) | bullet (tiles/tick) | ticks to empty + reload |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| pistol | 1 / 1 / 2 | 2 | 26 / 22 | 0 | 700 | 15.0 | 8.0 | 24 + 3 |
| rifle | 1 / 2 / 4 | 2 | 66 / 45 | 500 | 630 | 30.0 | 14.0 | 16 + 4 |
| smg | 1 / 1 / 1 | 1 | 24 / 8 | 200 | 700 | 16.0 | 9.0 | 16 + 3 |
| shotgun | 1 / 2 / 3 | 4 | 105 / 21 | 1560 | 665 | 9.0 | 8.0 | 18 + 5 |
| marksman | 2 / 6 / 6 | 5 | 74 / 63 | 0 | 560 | 40.0 | 20.0 | 23 + 5 |
| lmg | 2 / 6 / 6 | 3 | 80 / 45 | 1200 | 490 | 28.0 | 12.0 | 60 + 7 |
<!-- /generated:derived -->

Roles:

- **pistol**: the free sidearm, and a real gun. Exact single shots and a short startup even after running.
  It reaches 15 tiles and loses damage only from 12: four hits kill an unarmoured unit, five a light-armoured
  one and six a heavy one out to about 12.5 tiles, one more hit each at the edge of its reach. The bought
  guns earn their price by killing faster where they are meant to fight, not by out-ranging it.
- **smg**: cheap run-and-gun with the fastest fire rate, a pull every tick, and small hits. Running barely
  unsteadies it, so it starts as fast off a run as standing, and it carries at full speed; short reach, its
  two-pellet fan opens fast under sustained fire, and armour soaks many of its small hits.
- **shotgun**: seven pellets in a wide fan. One pull kills an unarmoured unit inside about three tiles and
  the fan is hard to sidestep; nearly harmless past six tiles. Slow pump, small magazine.
- **rifle**: the all-rounder. A tight three-bullet burst that kills in two pulls while all three land and
  still reaches across the map, if you stand still and pace your fire. It fires at the pistol's rate, slower
  than the smg and a braced lmg; its damage per pull is what sets it apart.
- **marksman**: one fast bullet out to 30 tiles, six past its own sight, so its longest shots need a teammate's
  spot. Two hits kill a bare or light-armoured unit and three a heavy-armoured one, anywhere in its reach: its
  shallow falloff never crosses a breakpoint. Slow to cycle, and a two-tick startup that the target can see,
  which grows the most of any weapon after moving.
- **lmg**: fast once braced. Five bullets a pull and, once it is firing, a pull every tick from a twenty-pull
  magazine, for holding a lane against a group; the fan opens as it keeps firing. Slow to carry, very slow to
  start after moving, and above all slow to reload.

## Time to kill

Measured by `crates/engine/tests/combat_ttk.rs` in the engine, not computed: ticks from the first `A_FIRE`
until a stationary target dies. The shooter stands, pulls whenever the weapon is ready and the pellets nearest
the aim would still land on the target, and reloads when empty. "After running" means it strafed across the
line of fire at the weapon's top speed for eight ticks just before the first pull, so that pull starts up
unsteady. "no kill" means the target was still alive after 100 ticks. Each distance has a standing and an
after-running column: 2 tiles is shotgun range, 5 and 10 are fighting distances, 15 is where the rifle meets the
marksman, and 20 is long range.

Target with 100 health and no armour:

<!-- generated:ttk -->
| weapon | 2 tiles standing | 2 tiles after running | 5 tiles standing | 5 tiles after running | 10 tiles standing | 10 tiles after running | 15 tiles standing | 15 tiles after running | 20 tiles standing | 20 tiles after running |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| pistol | 8 | 9 | 8 | 9 | 17 | 18 | no kill | no kill | no kill | no kill |
| rifle | 4 | 7 | 6 | 9 | 6 | 9 | 11 | 14 | 11 | 14 |
| smg | 6 | 6 | 6 | 6 | 9 | 9 | 19 | 19 | no kill | no kill |
| shotgun | 2 | 4 | 6 | 8 | no kill | no kill | no kill | no kill | no kill | no kill |
| marksman | 8 | 12 | 8 | 12 | 8 | 12 | 8 | 12 | 8 | 12 |
| lmg | 6 | 10 | 6 | 10 | 9 | 13 | 22 | 26 | 25 | 29 |
<!-- /generated:ttk -->

Target with 100 health and the lightest armour in the shop:

<!-- generated:ttk-light -->
| weapon | 2 tiles standing | 2 tiles after running | 5 tiles standing | 5 tiles after running | 10 tiles standing | 10 tiles after running | 15 tiles standing | 15 tiles after running | 20 tiles standing | 20 tiles after running |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| pistol | 10 | 11 | 10 | 11 | 19 | 20 | no kill | no kill | no kill | no kill |
| rifle | 4 | 7 | 8 | 11 | 8 | 11 | 13 | 16 | 15 | 18 |
| smg | 8 | 8 | 8 | 8 | 11 | 11 | 23 | 23 | no kill | no kill |
| shotgun | 6 | 8 | 10 | 12 | no kill | no kill | no kill | no kill | no kill | no kill |
| marksman | 8 | 12 | 8 | 12 | 8 | 12 | 8 | 12 | 8 | 12 |
| lmg | 6 | 10 | 6 | 10 | 9 | 13 | 28 | 32 | 31 | 35 |
<!-- /generated:ttk-light -->

Target with 100 health and the heaviest armour in the shop:

<!-- generated:ttk-armor -->
| weapon | 2 tiles standing | 2 tiles after running | 5 tiles standing | 5 tiles after running | 10 tiles standing | 10 tiles after running | 15 tiles standing | 15 tiles after running | 20 tiles standing | 20 tiles after running |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| pistol | 12 | 13 | 12 | 13 | 23 | 24 | no kill | no kill | no kill | no kill |
| rifle | 6 | 9 | 10 | 13 | 10 | 13 | 15 | 18 | 17 | 20 |
| smg | 10 | 10 | 10 | 10 | 15 | 15 | 29 | 29 | no kill | no kill |
| shotgun | 6 | 8 | 10 | 12 | no kill | no kill | no kill | no kill | no kill | no kill |
| marksman | 8 | 12 | 8 | 12 | 8 | 12 | 8 | 12 | 8 | 12 |
| lmg | 6 | 10 | 6 | 10 | 12 | 16 | 31 | 35 | 37 | 41 |
<!-- /generated:ttk-armor -->

These are duels against a target that stands still. A target that strafes at range dodges any single shot
aimed where it stood (the bullet takes time to arrive), so real fights take longer and reward leading.
