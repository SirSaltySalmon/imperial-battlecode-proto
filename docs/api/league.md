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

- `rating` is the rating system's displayed number, rounded. Weng-Lin (default) shows the conservative
  estimate `mu - 3 sigma` on a 1500-centred scale: a new team is about 760, a settled average team about
  1360, and 100 points is about 64 % expected score between settled teams. Elo shows the rating (start 1500).
- **Provisional**: a team with fewer than `IBC_PROVISIONAL_SERIES` (default 5) rated series shows
  `rating: null, tier: null, provisional: true` and is ranked after every rated team (among themselves by
  the hidden estimate).
- **Tiers** are bands of the displayed rating. Defaults (`IBC_TIER_FLOORS`, the floors of Bronze through
  Radiant, comma-separated, ascending):

  | Tier | Displayed rating |
  |---|---|
  | Iron | below 1100 |
  | Bronze | 1100 |
  | Silver | 1200 |
  | Gold | 1300 |
  | Platinum | 1400 |
  | Diamond | 1500 |
  | Ascendant | 1600 |
  | Immortal | 1700 |
  | Radiant | 1800 and up |

  Tiers are display only. The matchmaker pairs by rating and never looks at tiers.
- `games` counts rated maps, `series` rated series; `wins`/`losses`/`draws` are maps.

## How a ranked series is rated

A ranked series is `IBC_RANKED_GAMES` maps (default 3), each a full first-to-7 match, between both teams'
live bots at the time it is queued. Each map swaps attack and defence at half-time, and the A/B slot
alternates map by map from a coin flip on the series id, so an odd count is as fair as an even one. The scheduler runs every `IBC_RANKED_INTERVAL_SECS` (default 600; 0 turns
it off; `POST admin/ranked-batch` runs a round by hand).

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
