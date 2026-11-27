# League API: ladder, matches, series, scrims, maps

All under `/v1`. Errors are `{"error", "message"}`; a 429 adds `retry_after` (seconds) and a `Retry-After`
header. Read endpoints need no credentials; a team credential (Bearer `ibt…` team token, or whatever the
auth layer accepts for a team) only adds that team's own submissions to the match and series views.

## The archive is private

Every uploaded version is kept, but no public answer names another team's submission: no submission ids, no
module hashes, nothing that lets anyone count how many versions a team has fielded. The views below show
teams. A team sees the submission it played in its own series and its own side of a match summary; the
organisers (admin token) see both. The replay file itself (`GET matches/{id}/replay`) is redacted by the
replay-view endpoint, not here.

## `GET ladder`

```json
[{"rank": 1, "team_id": "t…", "team_name": "alpha", "rating": 1432, "tier": "Platinum",
  "provisional": false, "games": 42, "series": 14, "wins": 25, "losses": 15, "draws": 2}, ...]
```

One row per team with a live submission, computed from the stored ratings on each request.

- `rating` is the rating system's displayed number, rounded, for every team. Weng-Lin (default) shows its
  estimate `mu` on a 1500-centred scale: a new team starts at 1500, and 100 points is about 64 % expected
  score between settled teams. Elo shows the rating (start 1500). Rows are in rating order.
- **Provisional** means *placing*: the team's live submission has finished fewer than
  `IBC_PLACEMENT_SERIES` (default 3) ranked series. It shows its rating but `tier: null`. A new live
  submission plays these placement series back to back, each starting as soon as the last one ends, rather
  than one per scheduler round, so a new bot is placed within minutes. At most `IBC_PLACEMENT_DAILY_SERIES`
  (default 9) placement series start per team per 24 hours; past that, the team waits for the rounds.
- **Tiers** are bands of the displayed rating. Defaults (`IBC_TIER_FLOORS`, the floors of Bronze through
  Radiant, comma-separated, ascending) put the starting 1500 in the middle tier:

  | Tier | Displayed rating |
  |---|---|
  | Iron | below 1150 |
  | Bronze | 1150 |
  | Silver | 1250 |
  | Gold | 1350 |
  | Platinum | 1450 |
  | Diamond | 1550 |
  | Ascendant | 1650 |
  | Immortal | 1750 |
  | Radiant | 1850 and up |

  Tiers are display only. The matchmaker pairs by rating and never looks at tiers.
- `games` counts rated maps, `series` rated series; `wins`/`losses`/`draws` are maps.

## How a ranked series is rated

A ranked series is `IBC_RANKED_GAMES` maps (default 3), each a full first-to-7 match, between both teams'
live bots at the time it is queued. Each map swaps attack and defence at half-time, and the A/B slot
alternates map by map from a coin flip on the series id, so an odd count is as fair as an even one. A rematch
prefers the maps the two teams have not just played against each other.

The scheduler runs a round every `IBC_RANKED_INTERVAL_SECS` (default 600; 0 turns it off;
`POST admin/ranked-batch` runs a round by hand), and tops the queue up between rounds whenever the workers
have less than `IBC_RANKED_BACKLOG_SECS` (default 120) of work queued, so ranked games keep coming while the
fleet is idle. Opponents are near in rating, and ones met in the last 24 hours are less likely but never
ruled out. A team starts at most `IBC_RANKED_CAP_SERIES` (5) ranked series per `IBC_RANKED_CAP_WINDOW_SECS`
(900); when every team near its rating is at the cap, it may still play one of them, who then goes up to
`IBC_RANKED_CAP_OVERFLOW` (5) past it. So a team plays at most 10 ranked series in any 15 minutes.

When the series finishes, each map's Weng-Lin update is computed from the **pre-series** ratings and the
mean shifts are summed and applied once (`ibc_rating::rate_series`). A drawn map counts a half, the order the
maps finished in does not matter, a 3-0 moves more than a 2-1, and the round score inside a map is ignored.
Uncertainty shrinks as after that many games. A map that could not be run or ended unfinished is left out.
The ledger keeps one `series` event per rated series, so every rating can be recomputed.

## `GET matches?team=&kind=&series=&before=&limit=`

`{"matches": [...], "next": "<match id>" | null}`, most recent first. `team` is a team id or name, `kind` is
`ranked`, `unranked` or `tournament`, `limit` 1–500 (default 50). Page with `before=<next>` until `next` is
null. Each match: `match_id, series_id, index, kind, map, seed, bot_a {team_id, name}, bot_b, state, winner,
winner_team, outcome, score, ticks, replay_sha256, replay_url, verified, created_at, finished_at`.

`GET matches/{id}` adds `summary` (the runner's `summary.json`, with `teams[i].wasm_sha256` and
`teams[i].name` removed for every side the caller may not see). Admin also gets `job_id` and `error`.

## `GET series/{id}`

`{series_id, kind, team_a {team_id, name}, team_b, games, state, rated, wins_a, wins_b, draws, note,
created_at, finished_at, matches}`. `team_a`/`team_b` include `submission_id` and `sha256` only for the
caller's own team (admin: both).

## `POST scrims` (team)

```json
{"opponent_team_id": "beta", "games": 3, "maps": ["crossfire"], "sides": "alternate",
 "my_submission_id": "sub…", "against_series_id": "s…"}
```

Creates an unranked series and answers `201` with the series as the requester sees it.

- `opponent_team_id`: team id or name. Required.
- `games`: 1–5 maps, default 1.
- `maps`: pool map names, played in turn (cycling). Default: the pool, least-used maps first. Maps outside
  the pool are refused (404).
- `sides`: the requester's engine slot, `"a"`, `"b"` or `"alternate"` (default).
- `my_submission_id`: any of the requester's own validated submissions, live or archived. Default: live.
- `against_series_id`: a series both teams played (any kind). The opponent then plays the submission it
  fielded in that series; the server resolves it and never reveals which one. Without it the opponent is
  always its **current live bot**. Another team's archived versions are reachable in no other way.

Errors: 400 bad body, self-play or `games` out of range; 404 unknown team, map, own submission, or a series
the two teams did not both play (it does not say which); 409 `no_live_submission`, `not_validated`; 429
`quota_exceeded` when `games` does not fit the team's `IBC_UNRANKED_DAILY_GAMES` (default 100) over a rolling
24 h, with `retry_after` = seconds until enough earlier requests leave the window.

Organisers create series between live bots with `POST series {team_a, team_b, games, kind:
unranked|tournament}` (no quota); teams calling it get a 400 pointing here.

## `GET maps/{name}`

The `.ibcmap` text of a pool map (`text/plain`), for `ibc maps pull`. 404 for unknown maps and maps outside
the pool. `GET maps` lists them.
