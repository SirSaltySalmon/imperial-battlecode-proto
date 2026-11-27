# Tournaments API

Organisers run a tournament by hand: create a draft, lock it, start it, reveal its maps. Nothing schedules
one automatically. Code: `crates/coordinator/src/{api,db}/tournaments.rs`, bracket logic in
`crates/rating/src/bracket.rs` (pure, tested on its own).

## Lifecycle

| Step | Call | What happens |
|---|---|---|
| Draft | `POST /v1/admin/tournaments` | Stored with `status: "draft"`. Nothing else changes |
| Lock | `POST /v1/admin/tournaments/{id}/lock` | Field resolved to teams, each entrant's live submission and ladder seed snapshotted, map hashes fixed, ranked matchmaking paused. `status: "locked"` |
| Start | `POST /v1/admin/tournaments/{id}/start` | Bracket built, first pairings queued. `status: "running"`, then `"finished"` when the champion is decided |
| Reveal | `POST /v1/admin/tournaments/{id}/reveal` | Maps become public on the tournament and join the ranked pool. Any status; once |
| Resume ranked | `POST /v1/admin/tournaments/{id}/resume-ranked` | Lifts the ranked pause now (the pause is global) |

Every admin action answers with the admin view of the tournament (below). Wrong order: `409 wrong_status`.

### Create

```json
POST /v1/admin/tournaments
{"name": "Week 3", "format": "single" | "double",
 "field": {"kind": "all"} | {"kind": "top", "n": 16} | {"kind": "list", "team_ids": ["t..", "team-name"]},
 "maps": ["hidden1", "hidden2", "hidden3"]}
```

- `field` defaults to `all`. `top.n` is at least 2; a list needs two teams (ids or names, resolved to ids).
- `maps`: an odd number of distinct map names, each already uploaded. Upload hidden maps with
  `PUT /v1/admin/maps/{name}?in_pool=false` so ranked play does not use them.
- `400` for an even or empty map list, an unknown map or format; `404` for an unknown team.

### Lock

- **Seeds** come from the ladder at lock: rated teams by displayed rating, then provisional teams by displayed
  rating, ties by name.
- **Entrants**: `all` = every team with a live submission; `top` = the first `n` in seed order; `list` = the
  listed teams in seed order. Teams of the field without a live submission are left out and listed in
  `excluded` (`{team_id, name, reason}`). Fewer than two entrants: `409 not_enough_entrants`.
- **The snapshot is immutable**: the tournament plays the submission that was live at lock, whatever the team
  uploads or selects afterwards. Map hashes are fixed at lock too.
- **Ranked pause**: ranked matchmaking (the periodic scheduler and automatic `ranked-batch` rounds) creates no
  series until `IBC_TOURNAMENT_PAUSE_SECS` (default 3600) have passed or an organiser calls `resume-ranked`.
  Ranked series already running finish; unranked series and explicit `ranked-batch` pairings are not paused.
  The tournament keeps running after ranked play resumes; its jobs are in the `tournament` class, which always
  leases first.

### Start and advancing

- **Bracket**: padded to the next power of two, standard seeding (1 v N, 2 v N-1, ... placed so 1 and 2 meet
  only in the final), byes for the missing seeds. Double elimination adds a losers bracket of `2(k-1)` rounds
  and a grand final: the winners-bracket champion (seat `a`) against the losers-bracket champion (seat `b`).
  With two entrants double elimination is one match.
- **Bracket reset**: the winners-bracket champion has not lost yet. If it wins the grand final, it is champion
  and the tournament ends. If the losers-bracket champion wins, both teams have lost once and the reset, a
  second grand final between the same two teams in the same seats, is queued; its winner is champion and its
  loser second. The reset is in `rounds` from the start (`"Grand final reset"`, stage `final`, round 2):
  `waiting` until the grand final is decided, then `running` and `done`, or `empty` if it was not needed.
- **A pairing** is one `tournament` series: one game per tournament map, in map order, with the locked
  submissions; engine slots alternate from a coin flip on the series id. Tournament series are never rated.
- **Deciding a pairing** when its series closes: more maps won (a drawn map is half to each); if level, more
  rounds won across all its maps (regulation and overtime alike, from the result `score`); if still level, the
  higher seed advances and `decided_by` is `"seed"`, so organisers can filter for it. A map that could not be
  run or ended unfinished counts no map but keeps its rounds.
- **Games that could not be run** (dead-lettered: timed out on two machines, a missing blob, out of attempts):
  the pairing is played again, whole, as a new series, until it has used three series; then it is decided on
  the games that did finish (by seed, flagged, if none did).
- The next pairing is queued as soon as both of its seats are known.
- **Patch change mid-bracket**: a patch change dead-letters queued tournament jobs (`patch superseded`). The
  pairing is then played again, whole, as a new series on the current patch: a tournament series is never
  voided by a patch, and all maps of a pairing run on one patch.
- The first decision of a pairing stands: a series that a quarantine reopens later does not change the
  bracket.

### Hidden maps

Until a tournament is revealed, its maps (unless already in the ranked pool) are left out of `GET /v1/maps`,
`maps` is `null` in the public view, and its games are left out of `GET /v1/matches`, `GET /v1/matches/{id}`,
`GET /v1/matches/{id}/replay` and `GET /v1/series/{id}` (404), since names and replays would show the maps.
Bracket results are public throughout. The filter applies to the admin token too: until the reveal,
organisers inspect a pairing's games through `GET /v1/admin/jobs/{id}`.

## Reading

`GET /v1/tournaments` (no token):

```json
{"tournaments": [{"id", "name", "format", "status", "field", "winner", "maps_revealed", "entrants",
                  "created_at", "locked_at", "started_at", "finished_at", "revealed_at"}],
 "ranked_paused_until": "2026-..." | null}
```

`GET /v1/tournaments/{id}` (no token; the admin token adds `maps` before reveal and `submission_id` per entrant):

```json
{"id", "name", "format": "single" | "double", "status": "draft" | "locked" | "running" | "finished",
 "field", "winner": "<team id>" | null, "maps": ["..."] | null, "map_count": 3, "maps_revealed": false,
 "entrants": [{"seed": 1, "team_id", "name", "rating", "provisional"}],
 "excluded": [{"team_id", "name", "reason"}],
 "rounds": [{"name": "Winners round 1", "stage": "winners" | "losers" | "final", "round": 1,
             "pairings": [{"match": 0, "state": "waiting" | "running" | "done" | "bye" | "empty",
                           "a", "b", "a_name", "b_name", "seed_a", "seed_b", "series_id",
                           "maps_won": [2.5, 0.5] | null, "rounds_won": [19, 11] | null,
                           "winner": "<team id>" | null,
                           "decided_by": "maps" | "rounds" | "seed" | null}]}],
 "ranked_paused_until": "..." | null, "created_at", "locked_at", "started_at", "finished_at", "revealed_at"}
```

`a`/`b` are team ids (`null` for a seat not known yet or a bye); `seed_*` are 1-based. `rounds` is empty before
start. A `bye` pairing has a `winner` and no series; an `empty` one is a slot nobody reached: a losers-bracket
slot emptied by byes, or a bracket reset the winners-bracket champion made unnecessary by winning the grand
final.
