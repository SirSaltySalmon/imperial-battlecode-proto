# Bot ABI v0

The single source of truth is [`crates/abi/src/lib.rs`](../crates/abi/src/lib.rs): every id, record layout and
error code is defined and documented there. This page explains the conventions.

**The ABI is frozen.** Changes are additive only: new ids go at the end of their group, new record fields go at
the end of a record. Never renumber, reorder or change the meaning of an existing id or field.

## Shape

A bot is a `wasm32-wasip1` command module. One instance runs per unit and lives for the whole match. `_start`
loops forever; each iteration reads the world, queues intents and calls `end_turn`. Memory and stack persist.

Imports come from module `ibc`:

| Import | Purpose |
|---|---|
| `end_turn()` | Yield until this unit's next tick |
| `fuel_remaining() -> i64` | Fuel left this tick |
| `count(table) -> i32` | Rows in a table, or a negated error |
| `get(table, index, out, cap) -> i32` | Copy one row, or as much of it as fits in `cap`; returns bytes written, or a negated error |
| `act(kind, a, b, c) -> i32` | Queue an intent; returns 0 or an error code |
| `los(x0, y0, x1, y1) -> i32` | 1 if no wall lies between the points (static map only) |
| `raycast(x, y, angle, max) -> i32` | Distance to the first wall, capped at `max` |
| `mem_read(scope, off, ptr, len) -> i32`, `mem_write(...)` | Team, draft and buy memory (`MEM_*`; each writable in one phase) |
| `debug_draw(kind, x0, y0, x1, y1, color) -> i32` | Overlay in the replay viewer; free |
| `log(ptr, len) -> i32` | Append to the unit's log |

Only a small WASI preview1 subset exists: `fd_write` to stdout/stderr (captured, size-capped), a deterministic
clock and random source, empty args and environment. There is no filesystem, network or real time.

## Conventions

- **Tables, not functions.** Everything readable is a row in a table (`T_*`). Rows are `#[repr(C)]` records made
  only of `i32`, little-endian, no padding. `get` copies at most `cap` bytes of a record and returns the bytes
  written; a `T_MAP_ROW` buffer shorter than the row gets `ERR_BUFFER_TOO_SMALL`. Records only
  ever grow at the end, so a bot built against an older ABI keeps working: it reads the prefix it knows.
- **Fog is applied by the host, per team.** Vision and hearing are shared: `T_ENEMIES`, `T_PROJECTILES`,
  `T_EFFECTS`, `T_DROPS` and `T_SOUNDS` list what the caller's **team** perceives (the union over its living
  units), so every teammate reads the same rows. An enemy in `T_ENEMIES` may be behind a wall from where you
  stand; use `los` before shooting. Enemy rows hide `hp`, `armor`, credits, ammo, charges, cooldowns, blindness
  and the defuse kit; the weapon in hand is shown. Fog applies to what bots read, not to what replays record.
- **Events follow the fog.** `T_EVENTS` is filtered per team. A position is *known* to a team when one of its
  living units sees it, when the event happened to one of its own units, or when it is the charge's position and
  the team knows where the charge lies (both teams once it is planted, attackers when it is dropped). An event delivered
  with an unknown position carries `POS_HIDDEN` (`i32::MIN`) in both `x` and `y`, never `(0, 0)`.

  | Event | Delivered to a team | Position |
  |---|---|---|
  | `EVENT_KILL` (actor killer or -1, target victim, value weapon) | Always, both teams | `POS_HIDDEN` unless known |
  | `EVENT_DAMAGE` (actor attacker or -1, target, value amount) | Only when the position is known | Always real |
  | `EVENT_PLANT` (actor planter, value site) | Always | Real (the planted charge is public) |
  | `EVENT_DEFUSE` (actor defuser) | Always | Real |
  | `EVENT_DETONATE` | Always | Real |
  | `EVENT_ROUND_END` (actor winner, value `ROUND_WIN_*`) | Always | The charge position if known; `POS_HIDDEN` while it is carried |
  | Any later kind | Only when the position is known | Always real |

  Sounds are separate from events: a shot heard through a wall is a `T_SOUNDS` row, not a damage event.
- **Actions are intents.** `act` validates what it can immediately and returns an error code, but the action
  resolves with everyone else's when the tick is stepped. One move, one facing and one primary action per tick.
- **Rules are data.** Read `T_RULES_CORE`, `T_WEAPON`, `T_ABILITY`, `T_ITEM`, `T_KIT` instead of hard-coding numbers;
  balance patches change them without changing the ABI.
- **Reserved fields.** A field whose meaning is retired keeps its slot, is renamed `_reservedN` and always reads 0,
  so compiled bots keep their layout. `WeaponRules._reserved0` was `move_spread` (movement no longer adds spread).
- **Fuel.** Each host call costs its `FUEL_*` constant on top of the instructions executed.

## Additions

Additive changes since v0, each at the end of its group or record:

- `S_CASTING` (`1 << 12`): the unit is in an ability's startup (`AbilityRules.delay` ticks). Meanwhile `act`
  refuses every primary action except `A_CANCEL` with `ERR_NOT_READY`; moving and turning still work.
- `AbilityRules.speed`: units per tick a thrown device flies; 0 = it is not thrown and appears at once.
  `AbilityRules.delay` keeps its name and is the ability's startup.
- `ProjectileInfo.kind` (already in v0) is now used: 0 = a bullet, otherwise a thrown ability device with
  `kind` = ability id + 1 and `weapon` = -1. Devices hit nobody; they land at a wall, a barrier or their range.

## For engine and SDK authors

- New mechanics must fit the existing tables and action kinds. If you believe you need a new id, stop and raise
  it; do not edit `crates/abi` from a feature branch.
- The engine implements `ibc_abi::Host`. The wasm runner and the native sim both call only that trait, so a bot
  behaves the same in both.
