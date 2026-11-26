# ibc.h: C++ bot SDK for Imperial Battlecode

One bot instance runs per unit and lives for the whole match. Every tick it reads its (fogged) view and
queues intents. The SDK is two headers: `ibc_abi.h` (the frozen ABI: constants, records, raw imports) and
`ibc.h` (the typed API, float-free geometry, pathfinding). Both are embedded in the `ibc` binary, so a bot
directory needs nothing but its own sources.

## Quickstart

```sh
ibc toolchain install        # once: downloads the pinned wasi-sdk (sha256-verified) into your cache
ibc toolchain status         # or point IBC_WASI_SDK at an existing wasi-sdk 33
cp -r bots/cpp-starter bots/mybot
ibc build bots/mybot         # -> bots/mybot/.ibc/bot.wasm
ibc run bots/mybot bots/rusher --map maps/tunnels.ibcmap --seed 1 -o out/mine
```

A C++ bot is a directory with `src/*.cpp` (subdirectories are compiled too) and no `Cargo.toml`.
`#include "ibc.h"` just works; `src/` and an optional `include/` are on the include path.

```cpp
#include "ibc.h"
using namespace ibc;

struct MyBot {
    CoreRules core = core_rules();     // rules are data: read them, never hard-code them
    Navigator nav;                     // loads the map grid once

    void turn() {
        const UnitInfo me = ibc::me(); // fetch once per tick
        const MatchInfo info = match_info();
        if (!alive(me) || (info.phase != PHASE_ACTION && info.phase != PHASE_PLANTED)) return;
        for (const UnitInfo& e : enemies()) {            // only what THIS unit sees
            fire(angle_to(pos(me), pos(e)));
            return;
        }
        if (const auto site = sites().at(0)) {
            nav.go(me, {site->centre_x, site->centre_y}, core.run_speed);
            face(nav.heading);                           // you only see where you look
        }
    }
};
IBC_BOT(MyBot)
```

`bots/cpp-starter` is a complete bot (buy, path to a site, shoot with lead, plant, defuse) in about 200 lines.

## What is in `ibc.h`

| Area | API |
|---|---|
| Tables | `me()`, `allies()`, `enemies()`, `sounds()`, `projectiles()`, `effects()`, `drops()`, `events()`, `sites()`, `roster()`, `rounds()`, `items()`, `weapons()` (lazy ranges, no allocation); `match_info()`, `map_info()`, `core_rules()`, `weapon(id)`, `ability(id)`, `item(id)`, `kit(id)` |
| Actions | `move_dir`, `move_towards`, `face`, `fire`, `reload`, `plant`, `defuse`, `pickup`, `buy`, `buy_for`, `use_ability`, `swap`, `drop_item`, `draft`, `draft_for`, `cancel`: each returns an `Error` (`ok(e)`, `error_name(e)`) |
| World | `los(a, b)`, `raycast(p, angle, max)` |
| Memory | `team_mem_read/write`, `team_mem_get<N>/set<N>` (writes in action phases), `mem_read/write(scope, ...)`, `mem_get<N>/set<N>(scope, ...)` for `MEM_DRAFT` (draft tick) and `MEM_BUY` (buy phase) |
| Geometry | `angle_to`, `angle_diff`, `norm_angle`, `dist`, `dist2`, `len`, `isqrt`, `cos_sin`, `offset`, `lead`, `tile_of`, `tile_centre`: integer only, `constexpr`, bit-for-bit equal to the Rust SDK |
| Navigation | `Grid`, `Pathfinder` (cached distance fields, work capped per call), `Navigator::go` (path following with stuck recovery) |
| Debug | `log`, `logf`, `debug_line`, `debug_dot` |

Positions are fixed-point (`TILE` = 1024 per tile, +y down). Angles are 0..65535 per turn, 0 = +x,
increasing towards +y. Per tick a unit may queue one move, one facing and one "hands" action (fire, reload,
plant, defuse, pickup, ability, swap, drop, cancel); a later call of a kind replaces the earlier one.

## How it is built

`ibc build` calls wasi-sdk's `clang++` directly, once, with all sources:

```
--target=wasm32-wasip1 -std=c++20 -O2 -DNDEBUG -msimd128 -mbulk-memory -fno-exceptions -Wall
-Wl,--strip-all -Wl,-z,stack-size=1048576 --no-wasm-opt
```

`--debug` swaps `-O2 -DNDEBUG --strip-all` for `-O0 -g`. Exceptions are off by default because they make
the module bigger and every call slower. To opt in, add an `ibc.toml` next to `src/`:

```toml
[cpp]
exceptions = true    # -fwasm-exceptions -mllvm -wasm-use-legacy-eh=false, links the eh libc++ and libunwind
```

Note: the match runner does not enable the wasm exception-handling feature yet, so today an
exceptions build compiles and links but `ibc build` rejects the module ("exception refs not supported").
Until the runner turns it on, stay with the default. The SDK itself never throws and works either way. Floating point is allowed (the host canonicalises NaNs)
but the SDK does not use it. There is no filesystem, clock or network: WASI gives you `stdout`/`stderr`
(captured in the unit log), a deterministic clock and random source, and nothing else.

## Fuel tips

A unit gets `fuel_per_tick` wasm instructions per tick (more on the draft tick and in the buy phase). Run out
and the bot is paused mid-turn and resumes next tick, one tick late on everything.

- **`memcpy`/`memset` cost fuel per byte.** With `-mbulk-memory` they compile to `memory.copy`/`memory.fill`,
  which the host charges by length. Copying a 4 KB array every tick costs more than most turns. Pass
  records by `const&`, keep big state in members instead of rebuilding it, and do not zero buffers you are
  about to overwrite. (`T v{}` of a large struct is a `memset`.)
- **Host calls have a fixed price** (`FUEL_GET` 40, `FUEL_LOS` 60, `FUEL_RAYCAST` 80, ...). Fetch `me()` and
  `match_info()` once per tick; cache `core_rules()` and weapon rules, which never change during a match.
- **No heap in the turn loop.** `malloc`, `std::vector` growth, `std::string` and `std::function` all cost
  instructions and usually a `memcpy`. The SDK allocates only once per buffer (the grid, six distance
  fields); do the same: fixed arrays, `std::array`, `std::optional`.
- **Plan in the buy phase.** `Pathfinder::path_len` runs a whole search; `next_waypoint` and `Navigator::go`
  spread a new search over several ticks (160 tiles per call) and are safe mid-fight.
- **Keep `<iostream>` and `printf` out of release bots.** `logf` pulls in `vsnprintf` (about 10 KB of wasm
  and thousands of instructions per call); `log("text")` is one host call.
- **Integer geometry is cheap**: `angle_to` and `dist` are a few dozen instructions. There is no reason to
  reach for `atan2f`/`sqrtf`, and floats make it harder to reason about ties.
- Check `fuel_remaining()` before optional work, and read `fuel` in `summary.json` after a match.

## Keeping the ABI in sync

`ibc_abi.h` mirrors `crates/abi/src/lib.rs`. `cargo test -p ibc-cli` compares every constant, every record
field and every `static_assert(sizeof(...))` against the Rust crate, and compiles `tests/geom_test.cpp`
(golden checksums from the Rust geometry) when wasi-sdk is present. If the ABI grows, add the new ids and
fields to the header in the same order.
