# Playing a unit yourself: `ibc play`

`ibc play` lets you play in a team alongside its bot, against another bot, in the browser. Your team's bot
drafts and plays every unit; you take over one of them at a time and the bot plays the rest. You see what
your team could see, and you move, aim and shoot with the keyboard and mouse. It is for building a feel for
the game: how far vision reaches, how fast a unit turns, what a wind-up costs, how long a plant takes.

```
cargo build -p ibc-engine-wasm --release --target wasm32-unknown-unknown
(cd viewer && npm ci && npm run build)          # the play page is part of the viewer build
ibc play bots/holder bots/rusher --map maps/crossfire.ibcmap
```

It prints a URL (`http://127.0.0.1:<port>/`); open it. The bots draft, then time stops in the first buy
phase: pick the unit you want to play in the squad list (or with Tab), buy, and hold Space to start the
round.

## Options

| Option | Default | |
|---|---|---|
| `TEAM_BOT` | | your team's bot: it drafts and plays every unit you do not possess |
| `OPPONENT` | | the bot of the other team (a bot directory, built first, or a `.wasm`) |
| `--map M` | `maps/crossfire.ibcmap` | |
| `--seed S` | `0` | |
| `--mode active\|realtime\|turn` | `active` | see below |
| `--tps N` | `10` | ticks per second in the active and real-time modes (1 to 60) |
| `--team A\|B` | `A` | your team |
| `--unit SLOT` | none | the unit of your team you start in (0-based); without it you pick on the page |
| `--kit NAME` | the bot's pick | with `--unit`: the kit that unit drafts (a kit name from `rules/`) |
| `--port N` | any free port | |
| `-o, --out DIR` | `out/play` | where `replay.ibcr`, `summary.json` and the team logs are written |
| `--viewer-dir DIR` | as `ibc view` | the built viewer (`viewer/dist`), also `IBC_VIEWER_DIR` |

## Possessing a unit

The squad list shows every unit of your team with its kit and health. Click one (or press Tab for the next
living one) to take it over; click it again to give it back and only watch. The unit you leave goes back to
the bot at once: its bot did not run while you had the unit, and carries on from where it was. The draft is
the bot's, even for the unit you start in, unless `--kit` chose that unit's kit: then the session drafts it
at the unit's turn, like a bot would, and it is in the replay as that unit's act. When your unit dies, Tab
moves you to a living teammate.

## Modes

- **Active (default).** Time passes only while you act, at `--tps`. Moving, holding the trigger, planting or
  defusing, and one-shot actions in the action phases (reload, an ability, a weapon swap) each make the next
  ticks run; aiming alone does not, and neither do buys. Space waits: a press runs one tick, holding it lets
  ticks run at the clock's rate, which is how you get through the buy phase or let a fight play out. P lets
  time run on its own until you press it again. The bots' draft and the pause after a round run by
  themselves.
- **Real time.** Ticks run on a clock at `--tps`. Each tick uses whatever you are holding at that moment.
  The clock starts paused: Space (or P, or the button) starts and pauses it.
- **Turn based.** The match advances one tick each time you press Space. Hold keys and press Space to
  move a tick at a time; queued actions (a buy, a reload) go out with the next tick.

The page draws the motion between ticks smoothly (entity interpolation: the previous tick moving towards the
latest over one tick period), so units and bullets glide rather than jump. The picture trails the server by
at most one tick; the HUD and your controls always use the latest tick. The page hears of each tick as it
runs (`GET /state?after=N` answers when a newer tick exists), rather than polling on a timer.

Every bullet is drawn in flight, including one fired and stopped within a single tick (a point-blank shot
or one into a near wall), which no frame's `projectiles` ever holds: each frame also lists the `trails` of
the bullets that ended during its tick, from where they were when the tick began to where they stopped,
and the page flies them along that path. While a unit winds up a shot, a dashed line runs along its aim
to the weapon's range (`weapon_ranges` in the rules), or to the first wall in the way; a short bar marks
the end when the range, not a wall, is what stops it.

## Controls

| Input | Action |
|---|---|
| W A S D (or arrows) | move; hold Shift to walk (slower, no footstep sound) |
| Mouse | aim: your unit turns towards the pointer, at most `turn_rate` per tick |
| Left button (hold) | fire at the pointer |
| R | reload |
| Q / E | ability in slot 0 / 1, aimed at the pointer |
| F | pick up what is in reach (`A_PICKUP`: the charge first for an attacker, else the nearest weapon), once per press; held on, plant (attacking) or defuse (defending). The HUD prompts for whichever applies and counts a channel down |
| 1 / 2 | switch to weapon slot 0 / 1 |
| G | drop the weapon in hand (a weapon on the ground keeps the rounds in its magazine) |
| X | drop the charge |
| C | cancel the current channel |
| 1-9, 0 in the buy phase | buy the first ten items (the menu has all of them) |
| Tab | possess the next living unit of your team (the squad list also takes clicks) |
| Space | active mode: wait one tick, hold to keep waiting. Turn mode: one tick. Real time: start / pause |
| P | active and real-time modes: let time run on its own, or stop it |
| Wheel | zoom |

The HUD shows the phase and its clock, the score, your health, weapon and ammunition, credits, kit and
ability charges, and what the engine answered to your last acts ("fire: not ready", "buy: not enough
credits").

## Same rules as a bot

Your unit is not special. The page sends the state of your controls; once per tick the server turns them
into exactly the `act(kind, a, b, c)` calls a bot would make (`A_MOVE` with the run or walk speed, `A_FACE`
towards the pointer, one primary action such as `A_FIRE` or `A_RELOAD`, `A_BUY` in the buy phase) and makes
them through the same `ibc_abi::Host` the wasm runner uses, at your unit's place in the turn order, in place
of its bot's turn. The engine validates them as it validates any bot's: the turn rate limits how fast you face the
pointer, a wind-up is a wind-up, prices and phases apply. The controller reads your unit with the same
`T_SELF`, `T_MATCH` and `T_RULES_CORE` queries a bot has.

What you see is your team's view, built on the server: your team in full; enemies only while one of your
team sees them, as `T_ENEMIES` shows them (never their credits, ammunition or defuse kit); where an enemy
died only from the kill event of that tick (the page remembers it); enemy bullets, effects and drops only
where your team can see (an enemy bullet's trail if your team sees where it started or where it ended); the charge only when your side knows where it is. The page never receives the
rest: an enemy row it may not show is blanked and marked `"hidden": true`.

## Replays

When the match ends, or when you press Quit (`POST /quit`), `ibc play` writes the usual `replay.ibcr` and
`summary.json` to `--out`. Your acts are in the replay as your unit's acts, like a bot's, so

```
ibc replay verify out/play/replay.ibcr
ibc view out/play/replay.ibcr
```

re-simulate it exactly. Wall-clock time only decides when a tick runs, never what is in it. A match you
quit is recorded as unfinished. In the header your team is named `human+<team bot>`, with the team bot's
`sha256`.

## The server

Std only, bound to 127.0.0.1, like `ibc view`.

| Route | |
|---|---|
| `GET /` | the play page (`play.html` of the built viewer) |
| `GET /setup` | map text, rules, names of kits, abilities and items, your team (`team`, `team_size`, `first_unit`), the mode and `tps` |
| `GET /state` | after the latest tick: `view` (`frame` in the viewer's frame shape, fogged; `me`, null while watching; `match`), `possessed` (a slot or null), `last_acts`, `tick`, `running`, `done` |
| `GET /state?after=N` | the same, as soon as a tick after `N` has run (at most a 2 s wait) |
| `POST /input` | an input object (below) |
| `POST /step` | optional input object, then one tick; answers with the state after it |
| `POST /run` | `{"running": true\|false}`: let the clock run on its own, or stop it |
| `POST /possess` | `{"slot": n}`: take over that unit of your team from its bot; `{"slot": null}`: give it back |
| `POST /quit` | end the match here, write the replay and stop `ibc play` |

Every POST needs the header `X-IBC-Play: 1`, and every request a local `Host`. A page on another site can
neither send that header without a CORS preflight (never granted) nor read the answers.

An input object has held fields, which replace the previous value when present, and queued actions:

```json
{ "move": [1, -1], "walk": false, "aim": [11264, 5632], "fire": true, "interact": false, "wait": false,
  "actions": [{"act": "reload"}, {"act": "ability", "slot": 0}, {"act": "buy", "item": 3},
              {"act": "swap", "slot": 1}, {"act": "pickup"}, {"act": "drop", "kind": 2}, {"act": "cancel"}] }
```

`move` is a direction in screen axes (+x right, +y down), `aim` a world point in fixed-point units
(`"aim": null` clears it), `wait` lets time pass in active mode (Space held). Switching unit drops the queued
actions. In the action phases one queued action goes out per tick (the engine keeps one
primary action per tick), before held fire or interact; in the other phases all queued actions go out at
once.

## Code

| Path | |
|---|---|
| `crates/runner/src/lib.rs` | `Driver` and `run_driven`: the seam that lets units be driven from outside the runner, a tick at a time |
| `crates/cli/src/play.rs` | `ibc play`: options, setup, the match thread, writing the outputs |
| `crates/cli/src/play/input.rs` | input object -> acts (`Controller`) |
| `crates/cli/src/play/state.rs` | the fogged view |
| `crates/cli/src/play/session.rs` | shared state, possession, the tick clock of each mode, `PlayDriver` |
| `crates/cli/src/play/server.rs` | the HTTP routes |
| `viewer/play.html`, `viewer/src/play/` | the page: controls, HUD and squad, smooth drawing with the replay renderer |
