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
    --ranked-interval-secs 600
```

It speaks plain HTTP; put a TLS-terminating reverse proxy (Caddy, nginx) in front and allow request bodies
up to `--max-blob-bytes`. Every flag is also an environment variable (`IBC_DATA`, `IBC_ADMIN_TOKEN`, ...;
see `--help`). Ctrl-C shuts down gracefully; a kill is also safe (SQLite WAL, blobs are renamed into place).

Bring a league up (all with `Authorization: Bearer $ADMIN`):

```
POST /v1/admin/patch            {"engine": "0.1.0", "ruleset_hash": "<16 hex>", "wasmtime": "51.0.0"}   # = `ibc version --json`
PUT  /v1/admin/maps/tunnels     <bytes of maps/tunnels.ibcmap>          (?in_pool=false keeps it out of rotation)
POST /v1/admin/teams            {"name": "alpha"}                       -> {"team_id", "token"}  (token shown once)
POST /v1/submissions            <wasm bytes>, with the team's token     (?language=rust; admin: ?team=alpha)
POST /v1/admin/ranked-batch     {"pairings": [["alpha", "beta"]], "games": 2}     # or {} to pair by rating
```

### API

| | auth | |
|---|---|---|
| `POST /v1/workers/register`, `POST /v1/jobs/lease`, `POST /v1/jobs/{id}/heartbeat\|complete\|fail`, `GET\|PUT /v1/blobs/{sha256}` | worker (blobs: also admin) | protocol v1, see `docs/JOBS.md` |
| `GET /v1/team`, `POST /v1/submissions`, `GET /v1/submissions` | team | upload makes the module live if it validates |
| `POST /v1/series` | team: `{opponent, games}`; admin: `{team_a, team_b, games, kind: unranked\|tournament}` | unranked series, per-team daily quota (429 `quota_exceeded`) |
| `POST /v1/admin/teams`, `GET /v1/admin/teams`, `PUT /v1/admin/maps/{name}`, `POST /v1/admin/patch` | admin | |
| `POST /v1/admin/ranked-batch` | admin | explicit `pairings` or automatic (`budget`) |
| `POST /v1/admin/jobs` | admin | one stand-alone match (`class`, `bot_a`, `bot_b`, `map`, `seed`, `max_ticks`); answers `"cached": true` with the result if it already ran |
| `GET /v1/admin/jobs/{id}`, `GET /v1/admin/dead`, `POST /v1/admin/jobs/{id}/requeue` | admin | the dead-letter list |
| `GET /v1/admin/workers`, `POST /v1/admin/workers/{id}/trust`, `.../quarantine` | admin | |
| `GET /v1/ladder`, `/v1/matches?team=&series=&limit=&before=<next of the previous page>`, `/v1/matches/{id}`, `/v1/matches/{id}/replay`, `/v1/series/{id}`, `/v1/maps`, `/v1/patch` | none | |
| `GET /metrics`, `GET /healthz` | none | keep them off the public proxy if you care |

### How the pieces behave

- **Queue.** `lease` serves the lowest class first, oldest first; a job is promoted one class per
  `--aging-secs` waited, but never into the tournament class: tournament jobs always go first, and ranked,
  unranked and bench cannot starve one another. A lease lasts `--lease-secs` from grant and from each
  heartbeat; a sweeper requeues expired ones. Three leases without a result, or one `retryable: false`,
  and the job is on the dead-letter list until a human requeues it. A job that failed retryably is not
  handed back to the same worker for `--retry-grace-secs` (a worker on an old patch would otherwise burn
  all three attempts in milliseconds).
- **Idempotency.** A match is identified by the SHA-256 key of `docs/JOBS.md`. Enqueueing a key that exists
  returns that job; if it is done, that is a cache hit and no work is created.
- **Trust.** Workers registering with the trusted secret (or marked by an admin) get the `trusted` label; a
  self-declared `trusted` label is dropped. Results of untrusted workers are provisional: `--verify-fraction`
  of them (all `tournament` ones) are re-run as a `verify` job only trusted workers can lease. Equal replay
  hash: confirmed. Different: the trusted result replaces it, the worker is quarantined (no more jobs, leases
  revoked) and all its unconfirmed results are discarded and queued again. **Run at least one trusted worker
  whenever `--verify-fraction` is above 0**, or verifications pile up (`ibc_verify_pending`) and the series
  waiting on them stay open.
- **Series.** N games between the two teams' live submissions at creation: maps rotate through the pool,
  seeds come from the series id and game index, sides alternate. A series closes when none of its games and
  none of their verifications is outstanding. A ranked series is then rated game by game and a ladder
  snapshot is written; if either submission was superseded meanwhile the series is kept but not rated; a
  dead game voids the series. Setting a new patch dead-letters the jobs of the old one.
  Known limit: if a quarantine invalidates a result whose series was already rated, the match shows the
  re-run result but the rating change is not rolled back.
- **Scheduler.** Every `--ranked-interval-secs` (0 = off): live teams sorted by rating, neighbours paired
  (the offset alternates each round), at most `--ranked-series-budget` series, skipping teams that still
  have a ranked series running, which also bounds the backlog when workers are scarce.
- **Upload validation** is a light parser (`wasmparser`), not the runner: WebAssembly 2.0 only, size cap,
  initial-memory cap, imports only from `ibc` and the WASI functions the runner provides, exports `_start`
  and `memory`. The lists in `src/wasmcheck.rs` mirror `ibc_runner::{IBC_IMPORTS, WASI_IMPORTS}`; keep them
  in sync when the ABI grows. Maps are stored, not parsed: check them with `ibc` first.

### Ratings: where `ibc-rating` plugs in

`src/rating.rs` defines `RatingHook` (`initial()`, `update(a, b, outcome) -> (a', b')`, `display()`) with a
built-in Elo so the ladder works on its own. The `ratings` table already stores `mu` and `sigma`. To use
`ibc-rating`: implement `RatingHook` for its system (a five-line adapter), and in `main.rs` build the app
with `App::new(cfg, db, blobs, Arc::new(adapter))` instead of `App::open` (which hard-wires `Elo`).
Nothing else knows which system is in use.

## Data directory

```
<data>/coordinator.sqlite        everything relational (plus -wal / -shm next to it while running)
<data>/blobs/ab/cdef...          content-addressed blobs: bot modules, maps, replays (name = SHA-256)
<data>/blobs/tmp/                in-flight uploads, emptied at start
```

Tables: `teams`, `submissions` (one `live` per team), `maps`, `patches` (one `current`), `workers`, `jobs`
(payload, state, lease, result; `key` is unique), `series`, `games` (a game points at a job), `ratings`,
`ladder_snapshots` (the last 200).

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
rejected submissions, private scrimmages (all matches and replays are public), rolling back ratings.
Any registered worker can upload blobs, and the registration secret is shared: a quarantined worker can
register again, so rotate the secret when someone is caught.

## Later: Postgres and S3

- **SQL** is confined to `src/db/` (`mod.rs`, `jobs.rs`, `league.rs`): functions over a transaction. For
  Postgres, rewrite those three files against a pool; `Db::tx` becomes a real transaction, the lease query
  gains `FOR UPDATE SKIP LOCKED` (today the single connection is the lock), `json_extract/json_each` become
  `->>`/`jsonb_array_elements_text`, and the calls become async. The `Notify` used to wake long polls
  becomes `LISTEN/NOTIFY` if more than one coordinator process runs.
- **Blobs** are behind `blobs::BlobStore` (`put`, `get`, `exists`). An S3 implementation is one more
  `impl`; workers would then better get pre-signed URLs than proxy bytes through the coordinator, which is
  a protocol change (v2), not a storage one.
