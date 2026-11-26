# Replay viewer

A static web page that plays `.ibcr` replays. TypeScript and Canvas 2D, built with Vite, no UI framework and
no runtime dependencies. The built `dist/` is plain files: host it anywhere, or serve it with `ibc view`.

The viewer does not interpret replays itself. It loads the engine compiled to WebAssembly
(`crates/engine-wasm`), hands it the replay bytes, and the engine re-simulates the match, checks every state
hash and returns one JSON frame per tick. What you see is what the engine computed.

## Build and run

```
cargo build -p ibc-engine-wasm --release --target wasm32-unknown-unknown   # the engine module
cd viewer
npm install
npm test          # unit tests (vitest)
npm run build     # copies the engine into public/engine/, type-checks, writes dist/
```

Then either

```
ibc view out/e2e/replay.ibcr            # serves viewer/dist and the replay, prints the URL
ibc view replay.ibcr --port 8080 --viewer-dir path/to/dist
ibc view --replays out/replays         # a folder to pick from: menu in the header, [ and ] step through it
```

or any static server over `dist/` (`npx vite preview`, `python -m http.server`), opening
`/?replay=<url of an .ibcr>`. `npm run dev` runs the Vite dev server with hot reload.

`/editor.html` is the map editor: `ibc view --editor [--maps DIR]` serves it with lint and save for a map
folder (see `docs/maps.md`, "The editor"). From any other host it still opens, edits and exports maps.

`/lab.html` is the combat lab. It uses this viewer's theme, units and map to draw a recording from
`cargo run -p ibc-combatviz -- --out viewer/public/lab.json`. Drop a `lab.json` on the page if it is not
served from `public/`.

`/play.html` is the page of `ibc play` (play one unit against a bot; see `docs/PLAY.md`). It only works
served by `ibc play`, which provides its `/setup`, `/state` and input endpoints. Its code is in `src/play/`
and reuses the renderer, map parser and perspective of the replay viewer; it needs no engine module.

`ibc view` finds the built viewer through `--viewer-dir`, then `IBC_VIEWER_DIR`, then a `viewer/dist` beside
the working directory or the binary. The viewer is not embedded in the `ibc` binary on purpose: that would
make building `ibc` depend on Node.

The page must be served over HTTP(S); it does not work from `file://` (browsers block module scripts and
`fetch` there).

## Opening a replay

- With `ibc view --replays DIR` the header has a menu of every `.ibcr` under that folder (up to four levels
  down), labelled with the bots, the score and the map, and the empty page lists them as links. `[` and `]`
  open the previous and next one. The folder is read on every page load, so a match you run while the page
  is open appears after a reload. Each entry is an ordinary `?replay=replays/<path>` link.

- Drag an `.ibcr` file onto the page, or use **Open replay** (key `O`).
- `?replay=<url>` loads one on start. Optional extras, so a link can point at a moment:
  `tick=<n>`, `view=all|a|b|u<unit id>`, `speed=<x>`, `theme=dark|light`, `play=1`.

## Engine versions

A replay only re-simulates on the engine version that recorded it. `scripts/copy-engine.mjs` (run by
`npm run build`, or `npm run engine`) asks the built module for its version and copies it to
`public/engine/<engine_version>.wasm`, then rewrites `public/engine/index.json` from the files present.
Builds already in that directory are kept, so several versions sit side by side: to keep old replays
viewable, keep (or copy in) the `.wasm` of each released engine before building. Pass a path to add one:
`node scripts/copy-engine.mjs path/to/ibc_engine_wasm.wasm` (or set `IBC_ENGINE_WASM`). Without either it
reads the cargo target directory, `$CARGO_TARGET_DIR` when set.

On load the viewer reads the replay header with a small JS parser of the container (`src/ibcr.ts`), fetches
`engine/<engine_version>.wasm`, and reports a clear error if that version is not shipped. Errors from the
engine (corrupt file, ruleset mismatch, hash mismatch) are shown as the engine words them.

## Controls

| Key | Action |
|---|---|
| Space | play / pause |
| Left / Right | step one tick (Shift: ten) |
| Up / Down | faster / slower (0.25x to 16x; 1x is 8 ticks per second) |
| N / P | next / previous round |
| Home / End | start / end of the match |
| 1 / 2 / 3 | view as everyone / team A / team B |
| , / . | previous / next unit's view; Esc returns to everyone and closes the buy menu and team tab |
| V / S / D | vision cones / sounds / bot debug drawings |
| B | buy menu: price and stats of every weapon, ability, kit and armour item, and the economy |
| Tab | team tab: hold to peek, tap to pin it open (tap again to close). While a control has keyboard focus, Tab moves focus as usual |
| F | fit the map |
| T | dark / light theme |
| ? | key list |

Mouse: drag to pan, wheel to zoom, double-click to fit, click a unit (on the map or in the side panel) to see
through its eyes. The scrub bar shows a mark at each round start, coloured by the round's winner, and a
diamond at each plant.

## What is drawn

- Map: walls (solid), low cover (inset box with a slash: blocks movement, not sight or bullets), plant
  sites (lettered), spawns (dashed, in the colour of the team that holds that side this round).
- Units: team A is a solid blue disc, team B an orange ring, so teams differ by shape as well as colour.
  The stub is the facing. Bars above: health, and armour as the thin bar. Rings: yellow = winding up (with
  the aim line), grey spinner = reloading, pink arc = plant / defuse progress, dashed white with a slash =
  blinded, dotted yellow = revealed. The pink diamond is the charge. A cross is where a unit died.
- Bullets with a short trail, effects by kind (smoke, fire, barrier, flash, recon, heal, decoy), dropped
  weapons, the dropped or planted charge with its fuse, sounds heard this tick as rings.
- Kill feed (top right): the kills of the current round, killer, what killed (the weapon, or for a kill
  without a weapon the ability whose effect covered the victim), victim. Each entry stays 24 ticks (3 s at
  1x) of replay time and fades over the last 8, so pausing freezes it and scrubbing rebuilds it.
- Round history: a chip per decided round in the side panel (coloured by the winner, hover for how it was
  won), and as a list in the team tab. Only rounds decided by the tick shown are listed.
- Buy menu (B): built from the engine's rules export alone, generically: every record of the ruleset is a
  row and every field a column, so a weapon or a field added to `rules/*.toml` (and to
  `crates/engine-wasm/src/view.rs`) appears without a viewer change. Distances are in tiles (t), times in
  ticks (tk), angles in degrees.
- Team tab (Tab): both teams, every unit's kit, alive or dead, health out of the kit's maximum, armour, both
  weapons with ammo (the one in hand marked), ability charges and cooldowns, defuse kit, charge, credits,
  kills, deaths and damage, with the score and the round history. A replay holds the full state, so this
  shows what neither team saw of the other during the match.
- Perspectives: **Everyone** shows all and tints each unit's vision. **Team** and **unit** views fog the map
  outside the viewers' vision and hide enemies that are not in the viewers' `seen` bitmask, dead enemies whose
  bodies they cannot see, enemy bullets, effects and drops they cannot see, and other units' debug drawings
  and sounds. The side panel, kill feed
  and banner always show the full public state.

Between ticks, unit and bullet positions are interpolated for display only. Paused, the viewer shows exactly
the frame of that tick.

## Layout

| File | Role |
|---|---|
| `src/ibcr.ts` | `.ibcr` container: magic, version, inflate until the header line |
| `src/engine.ts` | engine module loading, `FrameStore` (frames parsed on demand, small cache) |
| `src/timeline.ts` | one-pass index: rounds, round results, public events |
| `src/map.ts` | map text parser, wall ray test |
| `src/vision.ts` | vision test and outline; rule numbers the frames do not carry |
| `src/perspective.ts` | what a perspective may show |
| `src/interp.ts` | in-between positions for units and bullets |
| `src/render.ts`, `src/draw-*.ts` | canvas drawing |
| `src/ui.ts`, `src/main.ts` | side panel, feed, banner, controls, input |
| `src/theme.ts` | colours for both themes |
| `src/panels/` | buy menu (`buy.ts`), team tab (`tab.ts`), kill feed (`killfeed.ts`), round history (`history.ts`), field formatting (`shared.ts`); `index.ts` wires them and their keys |

Besides the frame JSON of `ibc replay dump`, each frame from the engine module carries `loadout` (both
weapon slots, ammo per slot, the slot in hand, ability charges and cooldowns, defuse kit, damage dealt per
unit), and the rules export carries `core`, `economy`, `weapon_rules`, `ability_rules`, `kit_rules` and
`item_rules`; both are added in `crates/engine-wasm/src/view.rs`. An older engine build without them still
plays: the buy menu says it has no tables and the team tab shows only the weapon in hand.
| `src/play/` | the `ibc play` page: controls (key and mouse to input messages), HUD, server API |
| `src/editor/` | the map editor: `doc.ts` (map text), `paint.ts`, `transform.ts` (mirrors, rotations), `history.ts` (undo), `check.ts` (engine validation), `api.ts` (lint and save on `ibc view --editor`), `draw.ts` (through `render.ts`), `input.ts`, `panel.ts`, `main.ts` |

Frames stay inside the engine's memory as JSON strings; the viewer decodes a frame when it is shown and
keeps at most 48 parsed, so a long match costs little beyond the engine's own copy.

## Known gaps

The frame JSON and the header do not carry rules, so `src/vision.ts` holds defaults for the numbers the
viewer needs (`fov_half`, `vision_range`, `sense_radius`, `unit_radius`, `max_hp`, plant / defuse / fuse
ticks, weapon names), with a per-engine-version override table. If a ruleset changes those, update the table
until the engine exposes them. Reload progress is a spinner rather than a gauge for the same reason.
