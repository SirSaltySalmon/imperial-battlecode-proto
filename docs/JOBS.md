# Jobs: running matches at scale

Two layers. A **worker** leases match jobs from the **coordinator** over HTTPS (part B) and executes them
by feeding a local `ibc run-batch --jsonl` process (part A). A match is a pure function of
`(patch, map, bot A, bot B, seed)`, so any two honest workers produce byte-identical replays.

## A. `ibc run-batch --jsonl` (implemented in `crates/cli/src/batch.rs`)

```
ibc run-batch --jsonl [--jobs N]        # N parallel matches, default = number of cores
```

**stdin**: one JSON object per line, read until EOF. Blank lines are ignored. At most `N` unread jobs are
buffered, so a feeder gets back-pressure; the process may be kept alive and fed jobs as they arrive.

| field | type | |
|---|---|---|
| `id` | any JSON value | echoed unchanged in every event of the job |
| `bot_a`, `bot_b` | string | path of a `.wasm` file (team A, team B). Directories are not built in this mode |
| `map` | string | path of a `.ibcmap` file |
| `seed` | u64 | |
| `out_dir` | string | created if missing; receives `replay.ibcr`, `summary.json`, `team-a.log`, `team-b.log` |
| `fuel_mult_a`, `fuel_mult_b` | number, optional | language fuel multiplier, default 1.0, range 0.001..=1000, resolution 0.001 |
| `max_ticks` | u32, optional | stop early; the result is then `"outcome":"unfinished"` |
| `name_a`, `name_b` | string, optional | display names stored in the replay header. Default: first 12 hex digits of the module's SHA-256, so `replay_sha256` never depends on file paths. Workers must not set them |

**stdout**: only events, one JSON object per line, each written and flushed atomically. Events of different
jobs interleave; per job the order is `started` then exactly one of `finished` / `failed`.

```json
{"event":"started","id":"j1"}
{"event":"finished","id":"j1","winner":0,"outcome":"a","score":[7,3],"ticks":1840,"replay_sha256":"<64 hex>","summary_path":"out/j1/summary.json","replay_path":"out/j1/replay.ibcr","wall_ms":412}
{"event":"failed","id":"j2","error":"bot `x`: unsupported imports: env.foo"}
```

- `winner`: `0` (team A), `1` (team B) or `null`; `outcome`: `"a" | "b" | "draw" | "unfinished"`.
- `replay_sha256`: SHA-256 of the bytes of `replay.ibcr`; it is also in `summary.json`. `summary.json` is
  deterministic (no times, no paths) and is what a worker uploads as `result.summary`.
- `failed` means the match could not be run (unreadable file, invalid wasm or map, internal error). A bot
  that crashes mid-match is *not* a failure: its units are faulted and the match finishes normally.
- A line that is not a valid job produces `failed` with the line's `id` (or `null`) and no `started`.
- **stderr**: free-form logs. **Exit code**: 0 at EOF, whatever the job results were.

The text form `ibc run-batch <manifest> [--jobs N]` takes `botA botB map seed outdir` per line (`#`
comments, paths relative to the manifest, bots may be directories), prints human-readable lines and exits 1
if any match failed.

## B. Coordinator <-> worker lease protocol v1

HTTPS, JSON bodies (`Content-Type: application/json`), UTF-8. Times are RFC 3339 UTC strings. All SHA-256
values are 64 lower-case hex digits. Every request except `register` carries
`Authorization: Bearer <worker token>`. Errors are `{"error": "<code>", "message": "..."}` with status 400
(malformed), 401 (bad token), 404 (unknown job or blob), 409 (lease lost: wrong or expired `lease_token`;
the worker must drop the job), 429/503 (retry with exponential backoff, 1 s doubling to 60 s, with jitter).

### Endpoints

| | request body | 200 response |
|---|---|---|
| `POST /v1/workers/register` | `{registration_token, name, labels: [string], cores: int}` | `{worker_id, token}` |
| `POST /v1/jobs/lease` | `{max: int, wait_ms: int}` | `{jobs: [Job]}` |
| `POST /v1/jobs/{job_id}/heartbeat` | `{lease_token}` | `{lease_expires_at}` |
| `POST /v1/jobs/{job_id}/complete` | `{lease_token, result: Result}` | `{}` |
| `POST /v1/jobs/{job_id}/fail` | `{lease_token, error: string, retryable: bool}` | `{}` |
| `GET /v1/blobs/{sha256}` | | raw bytes (`application/octet-stream`) |
| `PUT /v1/blobs/{sha256}` | raw bytes | `{}` (201 if new, 200 if it already existed) |

`register`: `registration_token` is a shared secret handed out by the organisers. `token` is the bearer
worker token; it identifies the worker in every later call. `labels` are free-form (e.g. `"trusted"`,
`"x86_64-linux"`); the coordinator may use them for routing. Registering again yields a new id and token.

`lease`: long poll. The coordinator answers as soon as it has at least one job for this worker, with at
most `max` jobs, or with `{"jobs": []}` after `min(wait_ms, 25000)` ms. The worker then polls again.

```json
Job = {
  "job_id": "string", "lease_token": "string", "lease_expires_at": "2026-01-01T00:00:00Z",
  "kind": "match",
  "patch": {"engine": "0.1.0", "ruleset_hash": "<16 hex>", "wasmtime": "51.0.0"},
  "bot_a": {"sha256": "...", "fuel_mult": 1.0},
  "bot_b": {"sha256": "...", "fuel_mult": 1.0},
  "map": {"sha256": "..."},
  "seed": 12345,
  "max_ticks": 3000            // optional
}
Result = {"winner": 0 | 1 | null, "score": [int, int], "ticks": int,
          "replay_sha256": "...", "summary": { ...contents of summary.json... }}
```

`seed` is an unsigned 64-bit integer written as a JSON number; implementations must parse it without
going through a double. Unknown fields must be ignored; unknown `kind` values must be failed with
`retryable: true` (another worker may know them).

### Worker procedure

1. `patch` must equal the worker's own `ibc version --json` (`engine`, `ruleset_hash`, `wasmtime`). If not:
   `fail` with `error: "patch mismatch"`, `retryable: true`. Never run a job on a different patch.
2. Fetch `bot_a`, `bot_b` and `map` with `GET /v1/blobs/{sha256}` (cache by hash; verify the hash of what
   was received). A 404 is `fail` with `retryable: false`.
3. Run it (`run-batch --jsonl`, mapping `fuel_mult` to `fuel_mult_a/b`), sending `heartbeat` every 30 s.
4. On `finished`: `PUT /v1/blobs/{replay_sha256}` with the replay bytes, then `complete`. The coordinator
   rejects (400) a blob whose content does not hash to the path, and a `complete` whose replay blob is absent.
5. On `failed`: `fail`. `retryable: false` when the input itself is bad (invalid wasm or map), `true` for
   anything environmental (I/O error, crash, shutdown).

### Leases, retries, idempotency

- A lease lasts **180 s** from grant and from every successful heartbeat; workers heartbeat every **30 s**.
  A job whose lease expires returns to the queue. A 409 on heartbeat means it was reassigned: stop work.
- An **attempt** is one lease. After **3 attempts** that ended in expiry or `retryable: true`, or after one
  `retryable: false`, the job goes to the **dead-letter** list for a human; it is never retried silently.
- `complete` and `fail` are idempotent for the same `lease_token` (repeat calls return 200). `complete`
  with a stale lease returns 409 and is discarded.
- **Idempotency key** of a match job: `sha256(engine "\n" ruleset_hash "\n" wasmtime "\n" map.sha256 "\n"
  bot_a.sha256 "\n" bot_a.fuel_mult "\n" bot_b.sha256 "\n" bot_b.fuel_mult "\n" seed "\n" max_ticks-or-empty)`,
  numbers in shortest decimal form. Submitting a job whose key already exists returns the existing job (and
  its result if done) instead of queueing a second one. Team order matters: A-vs-B and B-vs-A are different.
- **Priority classes**: `tournament` (0) > `ranked` (1) > `unranked` (2) > `bench` (3). `lease` always hands
  out the lowest class number first, oldest job first within a class. No pre-emption of running jobs.

### Spot verification

Results from untrusted workers are provisional. The coordinator re-queues a random sample of completed
jobs (suggested: 5 %, and 100 % of `tournament`) restricted to workers labelled `trusted`, and compares
`replay_sha256`. Equal: the result is confirmed. Different: the trusted result wins, the original worker is
flagged and its unverified results are re-queued. Anyone holding a replay can also check it offline with
`ibc replay verify`, which re-simulates the inputs (it proves the replay is self-consistent, not that the
bots produced those inputs; only a re-run proves that).
