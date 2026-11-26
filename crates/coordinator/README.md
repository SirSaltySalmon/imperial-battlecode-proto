# ibc-coordinator

The always-on half of the match pipeline: it owns the job queue and hands matches to pull-based workers
over the lease protocol of [`docs/JOBS.md`](../../docs/JOBS.md) (part B, v1), and it owns teams, submissions,
series, ratings and the ladder. One process, one data directory, no other services.

The coordinator never runs a match and does not link the engine or wasmtime, so it does not have to be
redeployed when the game changes: the patch is three strings set through the API.

## Run it on one box

```
cargo build --release -p ibc-coordinator
target/release/ibc-coordinator --data /var/lib/ibc --listen 127.0.0.1:8080 \
    --admin-token "$ADMIN" --registration-token "$WORKER_SECRET" --trusted-registration-token "$TRUSTED_SECRET" \
    --ranked-interval-secs 600 --rating-system weng-lin
```

It speaks plain HTTP; put a TLS-terminating reverse proxy (Caddy, nginx) in front and allow request bodies
up to `--max-blob-bytes`. Every flag is also an environment variable (`IBC_DATA`, `IBC_ADMIN_TOKEN`, ...;
see `--help`). Ctrl-C shuts down gracefully; a kill is also safe (SQLite WAL, blobs are renamed into place).

Bring a league up (all with `Authorization: Bearer $ADMIN`):

```
POST /v1/admin/patch            {"engine": "0.1.0", "ruleset_hash": "<16 hex>", "wasmtime": "51.0.0"}   # = `ibc version --json`
PUT  /v1/admin/maps/tunnels     <bytes of maps/tunnels.ibcmap>          (?in_pool=false keeps it out of rotation)
POST /v1/admin/teams            {"name": "alpha"}                       -> {"team_id", "token"}  (token shown once)
POST /v1/submissions            <wasm bytes>, with the team's token     (?language=rust; ?live=false; admin: ?team=alpha)
POST /v1/admin/ranked-batch     {"pairings": [["alpha", "beta"]], "games": 2}     # or {} for the matchmaker
```

The patch may also name runner binaries for workers started with `--allow-runner-fetch` (upload the `ibc`
binary as a blob first): `{"engine": .., "ruleset_hash": .., "wasmtime": .., "runner_blobs":
{"linux-x86_64": "<sha256>"}}`. They are sent with the patch's jobs (`docs/JOBS.md`, "Runner blobs").

### API

| | auth | |
|---|---|---|
| `POST /v1/workers/register`, `POST /v1/jobs/lease`, `POST /v1/jobs/{id}/heartbeat\|complete\|fail\|release`, `GET\|PUT /v1/blobs/{sha256}` | worker (blobs: also admin) | protocol v1.1, see `docs/JOBS.md` |
| `POST /v1/auth/register\|login\|logout\|device\|device/approve\|device/token`, `GET /v1/me`, `POST /v1/teams`, `/v1/teams/join\|leave\|join-code/rotate`, `POST /v1/team/token` | see [`docs/api/auth.md`](../../docs/api/auth.md) | accounts, sessions, device login, teams of users |
| `GET /v1/team`, `POST /v1/submissions`, `GET /v1/submissions` | team | upload makes the module live if it validates, unless `?live=false` |
| `POST /v1/submissions/{id}/live` | team (its own), or admin | make a validated submission the live one |
| `POST /v1/scrims` | team | `{opponent_team_id, games, maps?, sides?, my_submission_id?, against_series_id?}`: an unranked series, per-team daily quota (429 `quota_exceeded` with `retry_after`); see `docs/api/league.md` |
| `POST /v1/series` | admin | `{team_a, team_b, games, kind: unranked\|tournament}`, no quota |
| `POST /v1/admin/teams`, `GET /v1/admin/teams`, `PUT /v1/admin/maps/{name}`, `POST /v1/admin/patch` | admin | |
| `POST /v1/admin/ranked-batch` | admin | explicit `pairings` or the matchmaker (`budget`, default `--ranked-series-budget`) |
| `POST /v1/admin/tournaments`, `POST /v1/admin/tournaments/{id}/lock\|start\|reveal\|resume-ranked`, `GET /v1/tournaments[/{id}]` | admin / none | tournaments, see `docs/api/tournaments.md`; a lock pauses ranked matchmaking for `--tournament-pause-secs` |
| `POST /v1/admin/jobs` | admin | one stand-alone match (`class`, `bot_a`, `bot_b`, `map`, `seed`, `max_ticks`); answers `"cached": true` with the result if it already ran |
| `GET /v1/admin/jobs/{id}`, `GET /v1/admin/dead`, `POST /v1/admin/jobs/{id}/requeue` | admin | the dead-letter list |
| `GET /v1/admin/workers`, `POST /v1/admin/workers/{id}/trust`, `.../quarantine` | admin | |
| `GET /v1/ladder`, `/v1/matches?team=&series=&kind=&limit=&before=<next of the previous page>`, `/v1/matches/{id}`, `/v1/matches/{id}/replay`, `/v1/series/{id}`, `/v1/maps`, `/v1/maps/{name}`, `/v1/patch` | none (a team token adds its own submissions to match and series views) | the archive is private: no other team's submission ids or hashes |
| `GET /metrics`, `GET /healthz` | none | keep them off the public proxy if you care |

### How the pieces behave

- **Queue.** `lease` serves the lowest class first, oldest first; a job is promoted one class per
  `--aging-secs` waited, but never into the tournament class: tournament jobs always go first, and ranked,
  unranked and bench cannot starve one another. A lease lasts `--lease-secs` from grant and from each
  heartbeat (jobs and heartbeat answers say so in `lease_secs`, and workers pace their heartbeats by it); a
  sweeper requeues expired ones. Three leases without a result, `--max-timeouts` (2) timeouts, or one
  `retryable: false`, and the job is on the dead-letter list until a human requeues it. A `release` gives the
  lease back without counting it. A job that was failed retryably or released is not handed back to the same
  worker for `--retry-grace-secs`.
- **Idempotency.** A match is identified by the SHA-256 key of `docs/JOBS.md`. Enqueueing a key that exists
  returns that job; if it is done, that is a cache hit and no work is created.
- **Trust.** Workers registering with the trusted secret (or marked by an admin) get the `trusted` label; a
  self-declared `trusted` label is dropped. Results of untrusted workers are provisional: `--verify-fraction`
  of them (all `tournament` ones) are re-run as a `verify` job only trusted workers can lease. Equal replay
  hash: confirmed. Different: the trusted result replaces it, the worker is quarantined (no more jobs, leases
  revoked) and all its unconfirmed results are discarded and queued again. A closed series that had counted
  one of them is reopened, its rating events are dropped and every rating is recomputed from the ledger;
  the re-runs then rate it again (see Ratings). **Run at least one trusted worker whenever
  `--verify-fraction` is above 0**, or verifications pile up (`ibc_verify_pending`) and the series waiting on
  them stay open.
- **Submissions and series.** A team may upload any number of modules (`?live=false` keeps the live one) and
  choose which one is live at any time (`POST /v1/submissions/{id}/live`); re-uploading the live module
  changes nothing. A series is created between the submissions live at that moment, and they are pinned:
  the series records them, its jobs name their hashes, and every game of it is played with them whatever is
  uploaded or selected later. A ranked series is always rated. Games: `ibc_rating::plan_series` picks the
  least-used maps of the pool (no repeat within a series), alternates the engine slots from a coin flip and
  derives the seeds from the series id. A series closes when none of its games and none of their
  verifications is outstanding; a ranked one is then rated once (see Ratings) and a ladder snapshot is
  written. A scrim may pin one of the requester's archived submissions, or the opponent's submission from a
  series both teams played (`against_series_id`), resolved on the server. A game that could not be run (dead-lettered) or ended `unfinished` is left out (the note says
  so); a series where no game finished is void. Setting a new patch dead-letters the jobs of the old one.
- **Scheduler.** Every `--ranked-interval-secs` (default 600; 0 = off) the matchmaker of `ibc-rating` (`plan_batch`,
  Halite III's scheme) pairs the live teams that have no ranked series running: under-played and idle teams
  are seeded first, opponents are drawn near in rating with a heavy tail, a pair meets at most twice a day.
  The round size comes from `series_per_batch`: the cores of the live workers, the mean `wall_ms` of the
  last hour's matches (its prior until there are some), 70 % utilisation, capped by
  `--ranked-series-budget`. No live workers, no series; while nothing is measured, at least one series a
  round, so the ladder starts by itself. Each series is `--ranked-games` maps (default 3), each a full
  match, between the teams' live bots. Each round's seed is kept in `ranked_batches`.
- **Placement.** A team whose live submission has finished fewer than `--placement-series` (3; 0 = off)
  ranked series is placing. While the scheduler is on, a task on the sweeper's period gives each placing
  team with no ranked series running one ranked series against the opponent `ibc_rating::plan_placement`
  picks (one of the three settled teams nearest in rating; the opponent may be busy). The next one starts
  as soon as it closes, so three series take minutes rather than three rounds. Placement series are marked
  in `series.placement_for`; at most `--placement-daily-series` (9) start per team per 24 h, which bounds
  what frequent uploads cost. A tournament lock pauses placement as it pauses the scheduler.
- **Upload validation** is a light parser (`wasmparser`), not the runner: WebAssembly 2.0 only, size cap,
  initial-memory cap, imports only from `ibc` and the WASI functions the runner provides, exports `_start`
  and `memory`. The lists in `src/wasmcheck.rs` mirror `ibc_runner::{IBC_IMPORTS, WASI_IMPORTS}`; keep them
  in sync when the ABI grows. Maps are stored, not parsed: check them with `ibc` first.

### Ratings

`--rating-system weng-lin` (default) or `elo`: the systems of `ibc-rating` with its defaults, behind
`RatingHook` (`src/rating.rs`; every `ibc_rating::RatingSystem` implements it). Nothing else knows which
system is in use.

- **Per series, from maps won.** A finished ranked series is one update (`ibc_rating::rate_series`): each
  map's update is computed from the **pre-series** ratings and the mean shifts are summed, a drawn map
  counting a half. Map order does not matter, a 3-0 moves more than a 2-1, and the round score inside a map
  does not matter. Each map's result is the job's `outcome` (`"a" | "b" | "draw"`, by engine slot, turned
  into the series' team A's view); an `"unfinished"` map is not rated, and not as a draw.
- **Computed once, stored.** Ratings are `f64` and may differ in the last bit across platforms, so they are
  computed here, as series are rated, and stored (`ratings`: `mu`, `sigma`, games, W/L/D, last played).
  Every change is also an event in the `rating_events` ledger: one `series` event per rated series (its map
  results in game order) and one `submission` event per change of live submission. A ledger from before
  ratings moved per series holds `game` events; they replay as one-map series. Ratings are a fold of the ledger and are recomputed from
  it, on the coordinator, in two cases only: a quarantine took back rated results (their events are dropped,
  the series is rated again from the re-runs, appended at the end), and `--rating-system` changed (at
  start; switching back gives the old numbers bit for bit).
- **Resubmission.** When a team's live submission is replaced (upload or selection), `on_new_submission`
  widens its uncertainty (Weng-Lin: `sigma = max(sigma, 0.85 sigma0)`; Elo reopens half the provisional
  window), and the new submission places (above). The estimate is kept as the starting point.
- **Ladder.** `GET /v1/ladder` is computed from the stored ratings on each request: an array of
  `{rank, team_id, team_name, rating, tier, provisional, games, series, wins, losses, draws}`, one per team
  with a live submission, in rating order. `provisional` means placing: the rating is shown, `tier` is null.
  Tiers (Iron .. Radiant) come from the displayed rating and `--tier-floors`; they are display only, the
  matchmaker never looks at them. Weng-Lin's displayed rating is `mu` on a 1500-centred scale, so a new
  team starts at 1500. Snapshots of the full table (with `mu`, `sigma` and the rating system's own
  `provisional` flag) are still written to `ladder_snapshots` for the organisers; they are not served.

For another system, implement `RatingHook` and build the app with `App::new(cfg, db, blobs, hook)`; give it
a new `name()`, since stored ratings are tagged with the system that produced them.

## Data directory

```
<data>/coordinator.sqlite        everything relational (plus -wal / -shm next to it while running)
<data>/blobs/ab/cdef...          content-addressed blobs: bot modules, maps, replays (name = SHA-256)
<data>/blobs/tmp/                in-flight uploads, emptied at start
```

Tables: `teams`, `submissions` (one `live` per team), `maps`, `map_usage` (games per map), `patches` (one
`current`, optional runner blobs), `workers`, `jobs` (payload, state, lease, result; `key` is unique),
`series` (with its pinned submissions), `games` (a game points at a job), `ratings`, `rating_events` (the
ledger ratings are a fold of), `ranked_batches` (matchmaker seeds), `settings` (the rating system that
produced the stored ratings), `ladder_snapshots` (the last 200). Columns added since the first release are
added to an existing data directory at start.

## Measured end to end

Real coordinator, two real workers (`--concurrency 4` each, one trusted), the prebuilt `ibc` 0.1.0, four
teams (rusher, holder, idle, rusher), three pool maps, all on the 20-thread Windows dev box: 968 ranked
games (242 series) completed in 7.6 s, about **127 matches/s** for 8 runner slots (mean `wall_ms` 36 ms per
match), plus 12 spot verifications, all confirmed. A worker killed with leases in hand: its jobs came back
after the lease (`--lease-secs`) and were finished by the other worker. `cargo test -p ibc-worker --test e2e`
(with `IBC_RUNNER` set) replays that scenario.

## Backup

Blobs are immutable: `rsync -a <data>/blobs/` at any time. For the database take a consistent copy while
running with `sqlite3 <data>/coordinator.sqlite ".backup backup.sqlite"` (or stop the process and copy the
file with its `-wal`). Copy the database first and the blobs second, so every hash the database mentions is
in the blob copy. Restore = put both back and start. Losing replays is recoverable in principle (a match is
a pure function of its job), losing submissions is not.

## Sizing

- CPU/RAM: the coordinator does no simulation. A lease or completion is one short SQLite transaction; a
  2-core, 1 GB box is enough for hundreds of workers. SQLite calls run inline on the async threads, which
  is fine while transactions stay in the sub-millisecond range.
- Long polls: one idle connection per polling worker, woken by a notification, re-checked once a second.
- Disk is what grows: one replay blob per match (tens to hundreds of kB) plus the summary JSON in the job
  row. 100k matches is on the order of 10-30 GB of replays. There is no garbage collection yet; old replays
  can be deleted from `blobs/` by hand (the match then 404s on replay download, everything else works).
- Autoscaling signals on `/metrics`: `ibc_queue_depth{class}`, `ibc_queue_oldest_age_seconds{class}`,
  `ibc_workers_live`, `ibc_workers_cores`, `ibc_jobs_per_minute`, `ibc_cache_hits_total`, `ibc_jobs_dead`.

## Not done here (deliberately)

Rate limiting and per-IP abuse protection (do it in the proxy), blob garbage collection, upload quotas for
rejected submissions, private scrimmages (all matches and replays are public), inactivity decay of ratings
(`on_inactive` is not called), widening every rating on a balance patch (`on_balance_patch`).
Any registered worker can upload blobs, and the registration secret is shared: a quarantined worker can
register again, so rotate the secret when someone is caught. A data directory from before the rating ledger
has ratings but no ledger: it keeps them, but a recompute (quarantine rollback, system switch) would start
from the ledger alone.

## Later: Postgres and S3

- **SQL** is confined to `src/db/` (`mod.rs`, `jobs.rs`, `league.rs`, `ratings.rs`): functions over a
  transaction. For Postgres, rewrite those files against a pool; `Db::tx` becomes a real transaction, the lease query
  gains `FOR UPDATE SKIP LOCKED` (today the single connection is the lock), `json_extract/json_each` become
  `->>`/`jsonb_array_elements_text`, and the calls become async. The `Notify` used to wake long polls
  becomes `LISTEN/NOTIFY` if more than one coordinator process runs.
- **Blobs** are behind `blobs::BlobStore` (`put`, `get`, `exists`). An S3 implementation is one more
  `impl`; workers would then better get pre-signed URLs than proxy bytes through the coordinator, which is
  a protocol change (v2), not a storage one.
