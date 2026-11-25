# ibc: Rust bot SDK for Imperial Battlecode

One bot instance runs per unit. Every tick it reads its (fogged) view and queues intents.
The same code builds to wasm for matches and runs natively in `ibc-sim` for fast debugging.

## Quickstart

```toml
# Cargo.toml
[package]
name = "mybot"
version = "0.1.0"
edition = "2021"
[workspace]
[dependencies]
ibc = { path = "../../sdk/rust" }
[profile.release]
panic = "abort"
opt-level = 3
lto = "fat"
codegen-units = 1
strip = true
```

```rust
// src/lib.rs: the bot (also usable natively by the simulator)
use ibc::*;

pub struct MyBot { core: CoreRules, nav: Navigator }

impl Bot for MyBot {
    fn new() -> Self { MyBot { core: core_rules(), nav: Navigator::new() } }

    fn turn(&mut self) {
        let me = me();                                   // fetch once per tick
        let info = match_info();
        if !me.alive() || !matches!(info.phase, PHASE_ACTION | PHASE_PLANTED) { return; }
        if let Some(e) = enemies().next() {
            let _ = fire(angle_to(e.x - me.x, e.y - me.y));
        } else if let Some(site) = sites().next() {
            let _ = self.nav.go(&me, site.centre_x, site.centre_y, self.core.run_speed);
            let _ = face(self.nav.heading);              // you only see where you look
        }
    }
}
```

```rust
// src/main.rs: the wasm entry point
ibc::bot!(mybot::MyBot);
```

```sh
cargo build --release --target wasm32-wasip1     # -> target/wasm32-wasip1/release/mybot.wasm
```

Positions are fixed-point (`TILE` = 1024 per tile, +y down). Angles are 0..65535 per turn, 0 = +x,
increasing towards +y. Read every number from the rules tables; never hard-code them.
Per tick a unit may queue one move, one facing and one "hands" action (fire, reload, plant, defuse,
pickup, ability, swap, drop, cancel); a later hands action replaces an earlier one. Moving cancels a
plant or defuse. Bullets leave along your facing, `windup` ticks after `fire`.

## API

| Area | Items |
|---|---|
| Entry | `trait Bot { new, turn }`, `bot!(Type)`, `end_turn()`, `fuel_remaining()` |
| Self and others | `me()`, `allies()`, `enemies()` (visible only), `sounds()`, `projectiles()`, `effects()`, `drops()`, `events()`, `roster()` (both teams: kit, round-start credits, alive), `rounds()` (finished rounds); `UnitExt`: `alive()`, `has(S_*)`, `pos()`, `busy()` |
| Match and map | `match_info()`, `map_info()`, `sites()`, `map_row(y, buf)`, `Grid::load()` with `tile`, `tile_at`, `is_wall`, `is_blocked`, `can_step`; `tile_of`, `tile_centre` |
| Rules | `core_rules()`, `weapon(id)`, `ability(id)`, `item(id)`, `kit(id)`, `weapons()`, `items()` |
| Actions (`Result<(), Error>`) | `move_dir(angle, speed)`, `move_towards(x, y, speed)`, `face(angle)`, `face_point(x, y)`, `fire(angle)`, `fire_at(x, y)`, `reload()`, `plant()`, `defuse()`, `pickup()`, `buy(item)`, `buy_for(item, slot)`, `use_ability(slot, x, y)`, `swap(slot)`, `drop_item(kind)`, `draft(kit)`, `draft_for(kit, slot)`, `cancel()`, raw `act(kind, a, b, c)` |
| World queries | `los(a, b)`, `raycast(x, y, angle, max)` |
| Team memory | `team_mem_read/write(off, bytes)`, `team_mem_get::<N>(off)` / `team_mem_set(off, [i32; N])` (writes in action phases), `mem_read/write(scope, ..)`, `mem_get::<N>(scope, off)` / `mem_set(scope, off, [i32; N])` for `MEM_DRAFT` (draft tick) and `MEM_BUY` (buy phase) |
| Geometry (integer only) | `angle_to(dx, dy)`, `angle_diff(a, b)`, `norm_angle`, `dist`, `dist2`, `len`, `isqrt`, `cos_sin`, `offset(x, y, angle, d)`, `lead(from, target, vel, bullet_speed, windup)` |
| Navigation | `Pathfinder` (`next_waypoint`, `path_len`; 8-connected, no corner cutting, one cached distance field per goal tile), `Navigator` (`go(&me, x, y, speed)`, `heading`, `path_len`; adds stuck detection) |
| Tactics (optional) | `tactics::Gunner` (`engage`, `top_up`), `tactics::shop`, `tactics::approach_point`, `tactics::hold_spot`, `target_half_angle`, `aim_error` |
| Debug | `log(&str)`, `log!(...)`, `debug_line(a, b, 0xRRGGBB)`, `debug_dot(p, color)` |
| Native only | `native::with_host(&mut host, unit, \|\| ...)`, `native::stats()`, `native::reset_stats()`, `native::set_log(true)` |

All ABI constants and records (`T_*`, `A_*`, `S_*`, `PHASE_*`, `UnitInfo`, `WeaponRules`, ...) are re-exported from `ibc_abi`.

## Running natively

```sh
cargo run --release -p ibc-sim -- run --a rusher --b holder --map maps/crossfire.ibcmap --seed 1
cargo run --release -p ibc-sim -- league --maps maps --seeds 3
```

`ibc_sim::run_custom` takes a closure that builds any `ibc::Bot`, so you can test your bot against the
reference bots with a normal debugger. Fuel is not modelled natively (`fuel_remaining()` is `i64::MAX`).

## Fuel tips

Fuel is counted in wasm instructions, refilled every tick (`CoreRules::fuel_per_tick`, multiplied on a
unit's first tick and in the buy phase). Running out ends your turn early.

- Host calls cost extra on top of the call itself: `get` 40, `count` 10, `act` 20, `los` 60, `raycast` 80,
  memory 20 (`FUEL_*`). Row iterators make one `count` and then one `get` per row actually consumed.
- Fetch `me()` and `match_info()` once per tick and pass them around; `move_towards`, `face_point` and
  `fire_at` each call `me()` again, so prefer `angle_to` plus the angle forms.
- Rules and the map never change: read `core_rules()`, weapons, sites and `Grid::load()` once, in `new` or
  on the first tick of a round. Do expensive planning in the buy phase.
- `memcpy`/`memset` cost fuel per byte: avoid cloning grids or big `Vec`s, do not collect iterators you
  can stream, and keep per-tick allocations out of hot loops.
- `Pathfinder` keeps 6 distance fields, one per goal tile, grown lazily from the goal. `next_waypoint` /
  `Navigator::go` settle at most 160 tiles per call (about 200 instructions each) and return `None` /
  walk straight until the search reaches you; `path_len` and `tactics::approach_point` finish the search
  in one call, so use those in the buy phase. Aim at a few stable goals, not a new tile every tick.
- `log!` formats a `String`: handy while debugging, wasteful in a submitted bot.
