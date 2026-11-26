# Playing a unit yourself: `ibc play`

`ibc play` lets you play one unit of a team against a bot, in the browser. You see what your team could
see, and you move, aim and shoot with the keyboard and mouse. It is for building a feel for the game: how
far vision reaches, how fast a unit turns, what a wind-up costs, how long a plant takes.

```
cargo build -p ibc-engine-wasm --release --target wasm32-unknown-unknown
(cd viewer && npm ci && npm run build)          # the play page is part of the viewer build
ibc build bots/holder
ibc play bots/holder --map maps/crossfire.ibcmap --mode turn
```

It prints a URL (`http://127.0.0.1:<port>/`); open it.

## Options

| Option | Default | |
|---|---|---|
| `OPPONENT` | | the bot of the other team (a bot directory, built first, or a `.wasm`) |
| `--map M` | `maps/crossfire.ibcmap` | |
| `--seed S` | `0` | |
| `--mode realtime\|turn` | `realtime` | see below |
| `--tps N` | `10` | ticks per second in real-time mode (1 to 60) |
| `--team A\|B` | `A` | your team |
| `--unit SLOT` | `0` | which unit of your team you play (0-based) |
| `--allies BOT` | none | the bot that runs your teammates; without it they stand still |
| `--port N` | any free port | |
| `-o, --out DIR` | `out/play` | where `replay.ibcr`, `summary.json` and the team logs are written |
| `--viewer-dir DIR` | as `ibc view` | the built viewer (`viewer/dist`), also `IBC_VIEWER_DIR` |

## Modes

- **Real time.** Ticks run on a clock at `--tps`. Each tick uses whatever you are holding at that moment.
  The clock starts paused so you can pick a kit: Space (or P, or the button) starts and pauses it.
- **Turn based.** The match advances one tick each time you press Space. Hold keys and press Space to
  move a tick at a time; queued actions (a buy, a reload) go out with the next tick.

## Controls

| Input | Action |
|---|---|
| W A S D (or arrows) | move; hold Shift to walk (slower, no footstep sound) |
| Mouse | aim: your unit turns towards the pointer, at most `turn_rate` per tick |
| Left button (hold) | fire at the pointer |
| R | reload |
| Q / E | ability in slot 0 / 1, aimed at the pointer |
| F (hold) | plant (attacking) or defuse (defending) |
| 1 / 2 | switch to weapon slot 0 / 1 |
| G / C | pick up / cancel the current channel |
| 1-9 in the draft | pick a kit (the menu also takes clicks) |
| 1-9, 0 in the buy phase | buy the first ten items (the menu has all of them) |
| Space | turn mode: one tick. Real time: start / pause |
| Wheel | zoom |

The HUD shows the phase and its clock, the score, your health, weapon and ammunition, credits, kit and
ability charges, and what the engine answered to your last acts ("fire: not ready", "buy: not enough
credits").

## Same rules as a bot

Your unit is not special. The page sends the state of your controls; once per tick the server turns them
into exactly the `act(kind, a, b, c)` calls a bot would make (`A_MOVE` with the run or walk speed, `A_FACE`
towards the pointer, one primary action such as `A_FIRE` or `A_RELOAD`, `A_BUY` and `A_DRAFT` in their
phases) and makes them through the same `ibc_abi::Host` the wasm runner uses, at your unit's place in the
turn order. The engine validates them as it validates any bot's: the turn rate limits how fast you face the
pointer, a wind-up is a wind-up, prices and phases apply. The controller reads your unit with the same
`T_SELF`, `T_MATCH` and `T_RULES_CORE` queries a bot has.

What you see is your team's view, built on the server: your team in full; enemies only while one of your
team sees them, as `T_ENEMIES` shows them (never their credits, ammunition or defuse kit); where an enemy
died only from the kill event of that tick (the page remembers it); enemy bullets, effects and drops only
where your team can see; the charge only when your side knows where it is. The page never receives the
rest: an enemy row it may not show is blanked and marked `"hidden": true`.

## Replays

When the match ends, or when you press Quit (`POST /quit`), `ibc play` writes the usual `replay.ibcr` and
`summary.json` to `--out`. Your acts are in the replay as your unit's acts, like a bot's, so

```
ibc replay verify out/play/replay.ibcr
ibc view out/play/replay.ibcr
```

re-simulate it exactly. Wall-clock time only decides when a tick runs, never what is in it. A match you
quit is recorded as unfinished. In the header your team is named `human` (or `human+<allies bot>`); a team
none of whose units ran a module has an empty `sha256`.

## The server

Std only, bound to 127.0.0.1, like `ibc view`.

| Route | |
|---|---|
| `GET /` | the play page (`play.html` of the built viewer) |
| `GET /setup` | map text, rules, names of kits, abilities and items, your unit and team, the mode |
| `GET /state` | after the latest tick: `view` (`frame` in the viewer's frame shape, fogged; `me`; `match`), `last_acts`, `tick`, `running`, `done` |
| `POST /input` | an input object (below) |
| `POST /step` | optional input object, then one tick; answers with the state after it |
| `POST /run` | `{"running": true\|false}`: start or pause the real-time clock |
| `POST /quit` | end the match here, write the replay and stop `ibc play` |

Every POST needs the header `X-IBC-Play: 1`, and every request a local `Host`. A page on another site can
neither send that header without a CORS preflight (never granted) nor read the answers.

An input object has held fields, which replace the previous value when present, and queued actions:

```json
{ "move": [1, -1], "walk": false, "aim": [11264, 5632], "fire": true, "interact": false,
  "actions": [{"act": "reload"}, {"act": "ability", "slot": 0}, {"act": "buy", "item": 3},
              {"act": "draft", "kit": 2}, {"act": "swap", "slot": 1}, {"act": "pickup"},
              {"act": "drop", "kind": 2}, {"act": "cancel"}] }
```

`move` is a direction in screen axes (+x right, +y down), `aim` a world point in fixed-point units
(`"aim": null` clears it). In the action phases one queued action goes out per tick (the engine keeps one
primary action per tick), before held fire or interact; in the other phases all queued actions go out at
once.

## Code

| Path | |
|---|---|
| `crates/runner/src/lib.rs` | `Driver` and `run_driven`: the seam that lets units be driven from outside the runner |
| `crates/cli/src/play.rs` | `ibc play`: options, setup, the match thread, writing the outputs |
| `crates/cli/src/play/input.rs` | input object -> acts (`Controller`) |
| `crates/cli/src/play/state.rs` | the fogged view |
| `crates/cli/src/play/session.rs` | shared state, the tick clock, `PlayDriver` |
| `crates/cli/src/play/server.rs` | the HTTP routes |
| `viewer/play.html`, `viewer/src/play/` | the page: controls, HUD, drawing with the replay renderer |
