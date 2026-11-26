# `ibc` and the platform

The same `ibc` binary that builds and runs bots locally also talks to the platform: sign in, submit, choose the
live version, request unranked scrims, read the ladder and matches, download replays and the map pool. Every
command is meant to be driven by a script or an AI agent as easily as by hand.

## Signing in

```
ibc login --server https://platform.example     # device code: open the printed URL, enter the code
ibc login --server https://platform.example --token ibt_…   # or paste a team token (ibt…) or user token (ibu…)
ibc whoami
ibc logout
```

`ibc login` without `--token` runs the device flow: it prints a verification URL and a short code on stderr,
then polls until you approve the code in the browser (or it expires). The token it receives, or the one you
pasted, is checked against the server before it is saved.

Credentials live in `credentials.json` in the per-user config directory: `%APPDATA%\ibc` on Windows,
`$XDG_CONFIG_HOME/ibc` or `~/.config/ibc` elsewhere (`IBC_CONFIG_DIR` overrides it). The file holds the server
URL and the token:

```json
{ "server": "https://platform.example", "token": "ibu_…" }
```

| Setting | Order of precedence |
|---|---|
| Server | `--server URL` > `IBC_SERVER` > the stored server |
| Token | `IBC_TOKEN` > the stored token, **only** if the chosen server is the one it was stored for |

So `--server` pointed somewhere else never leaks your stored token; set `IBC_TOKEN` to use a token with any
server (CI, several accounts).

## Commands

| Command | What it does |
|---|---|
| `ibc submit BOT_DIR [--live] [--note TEXT]` | Builds the bot in release mode (`ibc build`), uploads `.ibc/bot.wasm`, prints the submission id, sha256 and validation result. Without `--live` the new version is stored but not played; a prebuilt `.wasm` path is uploaded as is (`--language`, default `rust`). |
| `ibc submissions [--live ID]` | Lists your team's submissions (`*` marks the live one). `--live ID` makes an older one live without re-uploading. |
| `ibc ladder [--top N]` | The ladder. |
| `ibc matches [--mine \| --team T] [--series S] [--kind ranked\|unranked\|tournament] [--limit N] [--before ID]` | Matches, most recent first. A full page prints the `--before` value of the next one. |
| `ibc scrim OPPONENT [--games N] [--maps a,b,c] [--me SUBMISSION] [--rematch SERIES]` | Requests an unranked series against another team, from your daily quota. `--me` plays one of your older submissions; `--rematch` plays against the opponent bot of an earlier series. |
| `ibc replay download MATCH_ID [-o FILE] [--view public\|team]` | Saves the replay (default `out/replays/<MATCH_ID>.ibcr`) and reads its header. Then `ibc replay verify FILE` or `ibc view FILE`. |
| `ibc maps pull [--dir maps] [--all]` | Writes the platform's map pool as `<dir>/<name>.ibcmap`, checking each file's sha256 against the list. Files are only created or replaced under pool names; your own maps are never deleted. `--all` includes maps retired from the pool. |

## For scripts and agents

**`--json`.** Every platform command takes `--json` and then prints exactly one JSON document on stdout. Human
progress (the device code prompt, "building…", notes) goes to stderr. The shapes:

| Command | stdout |
|---|---|
| `login`, `whoami` | `{server, token_kind: "user"\|"team", user: {id,email,display_name,is_admin}\|null, team: {id,name,…}\|null}`; `login` adds `saved_to` |
| `logout` | `{logged_out, path}` |
| `submit` | `{module: {path, sha256, language, bytes}, submission: <server answer: submission_id, sha256, status, live, …>}` |
| `submissions` | `{submissions: [{submission_id, sha256, size, language, status, reason, live, created_at, …}]}`; with `--live`: `{team_id, live_submission, changed}` |
| `ladder` | `{generated_at, teams: [{rank, team_id, team_name, rating, provisional, games, …}]}` |
| `matches` | `{matches: [{match_id, series_id, kind, map, bot_a: {team_id,name}, bot_b, state, outcome, score, replay_url, …}], next}` |
| `scrim` | the series: `{series_id, kind, team_a, team_b, games, state, matches, …}` |
| `replay download` | `{match_id, path, bytes, sha256, view, engine_version, result}`, or `decode_error` instead of the last two when this `ibc` cannot read the replay (it is saved anyway) |
| `maps pull` | `{dir, maps: [{name, path, sha256, status: "new"\|"updated"\|"unchanged"\|"failed", error?, message?}]}` |
| any failure | `{error, message, status, retry_after}` |

**Errors.** A failure prints the server's `{error, message}` (on stderr, or as the JSON above). `error` is the
server's code (`bad_request`, `invalid_wasm`, `quota_exceeded`, `rate_limited`, …) or a local one: `usage`,
`auth_required`, `network`, `endpoint_missing` (the server is older than this `ibc` and lacks the endpoint),
`bad_response`, `build_failed`, `error`.

**Rate limits.** A 429 prints how long to wait (from `Retry-After`, or the body's `retry_after`). Add `--wait` to
sleep that long and retry once (up to an hour).

**Exit codes.**

| Code | Meaning |
|---|---|
| 0 | ok |
| 1 | the server refused (4xx/5xx, rate limited), the network failed, or a local step failed (build, file) |
| 2 | usage: bad arguments or flags |
| 3 | sign-in needed: no server or token configured, the server answered 401, or the device code expired |

## API used

All under `/v1` with `Authorization: Bearer <token>`: `POST auth/device`, `POST auth/device/token`,
`POST auth/logout`, `GET me`, `GET team`, `POST submissions?language=&live=&note=`, `GET submissions`,
`POST submissions/{id}/live`, `GET ladder`, `GET matches?team=&kind=&series=&before=&limit=`, `POST scrims`
(falls back to `POST series` on a server without it, when no scrim-only option is given),
`GET matches/{id}/replay?view=`, `GET maps`, `GET maps/{name}`.
