# Jobs: running matches at scale

Two layers. A **worker** leases match jobs from the **coordinator** over HTTPS (part B) and executes them
by feeding a local `ibc run-batch --jsonl` process (part A). A match is a pure function of
`(patch, map, bot A, bot B, seed)`, so any two honest workers produce byte-identical replays.

Accounts, sessions, teams of users and the credentials team routes accept are in
[`docs/api/auth.md`](api/auth.md).

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

**Revision 1.1** (marked *1.1* below) only adds optional fields and one endpoint; peers of either revision
interoperate. A 1.1 coordinator uses the summary's `outcome` when a 1.0 worker sends none; a 1.1 worker whose
`release` gets a 404 from a 1.0 coordinator sends `fail` with `retryable: true` instead.

### Endpoints

| | request body | 200 response |
|---|---|---|
| `POST /v1/workers/register` | `{registration_token, name, labels: [string], cores: int}` | `{worker_id, token}` |
| `POST /v1/jobs/lease` | `{max: int, wait_ms: int}` | `{jobs: [Job]}` |
| `POST /v1/jobs/{job_id}/heartbeat` | `{lease_token}` | `{lease_expires_at, lease_secs}` |
| `POST /v1/jobs/{job_id}/complete` | `{lease_token, result: Result}` | `{}` |
| `POST /v1/jobs/{job_id}/fail` | `{lease_token, error: string, retryable: bool, code?: string}` | `{}` |
| `POST /v1/jobs/{job_id}/release` *1.1* | `{lease_token, reason: string}` | `{}` |
| `GET /v1/blobs/{sha256}` | | raw bytes (`application/octet-stream`) |
| `PUT /v1/blobs/{sha256}` | raw bytes | `{}` (201 if new, 200 if it already existed) |

`register`: `registration_token` is a shared secret handed out by the organisers. `token` is the bearer
worker token; it identifies the worker in every later call. `labels` are free-form (e.g. `"x86_64-linux"`);
the coordinator may use them for routing. Registering again yields a new id and token.

**Trust** comes from the secret, not from a label: the organisers hand out a second, *trusted* registration
secret, and a worker registering with it gets the `trusted` label (results final, runs spot verifications,
see below). A `trusted` label a worker declares itself is ignored. The coordinator may also grant or revoke
trust by hand.

`lease`: long poll. The coordinator answers as soon as it has at least one job for this worker, with at
most `max` jobs, or with `{"jobs": []}` after `min(wait_ms, 25000)` ms. The worker then polls again.

```json
Job = {
  "job_id": "string", "lease_token": "string", "lease_expires_at": "2026-01-01T00:00:00Z",
  "lease_secs": 180,           // 1.1, optional: the lease length this coordinator grants
  "kind": "match",
  "patch": {"engine": "0.1.0", "ruleset_hash": "<16 hex>", "wasmtime": "51.0.0"},
  "bot_a": {"sha256": "...", "fuel_mult": 1.0},
  "bot_b": {"sha256": "...", "fuel_mult": 1.0},
  "map": {"sha256": "..."},
  "seed": 12345,
  "max_ticks": 3000,           // optional
  "runner_blobs": {"linux-x86_64": "<sha256>"}   // 1.1, optional, see "Runner blobs"
}
Result = {"winner": 0 | 1 | null,
          "outcome": "a" | "b" | "draw" | "unfinished",   // 1.1; the coordinator falls back to summary.outcome
          "score": [int, int], "ticks": int,
          "replay_sha256": "...", "summary": { ...contents of summary.json... },
          "wall_ms": int}                                 // 1.1, optional: the runner's wall_ms
```

`seed` is an unsigned 64-bit integer written as a JSON number; implementations must parse it without
going through a double. Unknown fields must be ignored; a job of an unknown `kind` is given back with
`release` (another worker may know it; a 1.0 worker `fail`s it with `retryable: true`).

`outcome` is what the ladder is rated from: `winner` is `null` both for a draw and for a match stopped at
`max_ticks` (`"unfinished"`), and an unfinished game must not be rated as a draw. It must agree with
`winner` (`"a"` with 0, `"b"` with 1, `"draw"`/`"unfinished"` with `null`); the coordinator rejects (400) a
result where it does not. `wall_ms` is not part of the match's determinism (it is not compared); the
coordinator uses it to size ranked batches to the fleet.

The ladder is rated per series, not per job: when every map of a ranked series has finished, the series'
`outcome`s (maps won, lost, drawn) move the two ratings once, from the pre-series ratings
(`docs/api/league.md`). A ranked series is `IBC_RANKED_GAMES` maps (default 3), queued by the built-in
scheduler every `IBC_RANKED_INTERVAL_SECS` (default 600, 0 = off) with each team's live bot.

### Worker procedure

1. `patch` must equal the worker's own `ibc version --json` (`engine`, `ruleset_hash`, `wasmtime`), or a
   side-by-side runner's. If not: `release` with `reason: "patch mismatch"`. Never run a job on a different
   patch.
2. Fetch `bot_a`, `bot_b` and `map` with `GET /v1/blobs/{sha256}` (cache by hash; verify the hash of what
   was received). A 404 is `fail` with `retryable: false`.
3. Run it (`run-batch --jsonl`, mapping `fuel_mult` to `fuel_mult_a/b`), sending `heartbeat` six times per
   lease: every `lease_secs / 6` (30 s for 180 s). A worker that got no `lease_secs` assumes 180 s.
4. On `finished`: `PUT /v1/blobs/{replay_sha256}` with the replay bytes, then `complete`. The coordinator
   rejects (400) a blob whose content does not hash to the path, and a `complete` whose replay blob is absent.
5. On `failed`: `fail`. `retryable: false` when the input itself is bad (invalid wasm or map), `true` for
   anything environmental (I/O error, crash).
6. A match that runs past the worker's wall-clock limit is killed and failed with `retryable: true,
   code: "timeout"`.
7. Shutting down with a job in hand (drain timeout, second signal): `release` it, `reason: "worker shutting
   down"`.

### Leases, retries, idempotency

- A lease lasts **180 s** (`lease_secs`) from grant and from every successful heartbeat; workers heartbeat
  every `lease_secs / 6` (**30 s**). The lease length is measured from when the worker asked, on its own
  clock: `lease_expires_at` is informative only, so clock skew between machines does not matter. A job whose
  lease expires returns to the queue. A 409 on heartbeat means it was reassigned: stop work.
- An **attempt** is one lease. After **3 attempts** that ended in expiry or `retryable: true`, or after one
  `retryable: false`, the job goes to the **dead-letter** list for a human; it is never retried silently.
- *1.1* **Timeouts.** A match is deterministic, so a timeout is likely to repeat wherever it runs: a job is
  dead-lettered on its **second** `code: "timeout"` failure (the first is retried, on another worker, in
  case the first machine was overloaded) instead of burning every attempt. Other `code` values are ignored.
- *1.1* **Release** ("not for me") returns a leased job to the queue **without counting the attempt**: the
  job is fine, this worker cannot serve it (another patch, an unknown kind) or is going away. A job no fleet
  worker can serve therefore waits in the queue (visible in the queue-age metric) instead of dying.
- **Retry grace.** A job that was failed with `retryable: true` or released is not handed back to the same
  worker for **30 s** (suggested), so a worker on the wrong patch cannot spin through a job, and a flaky
  machine's job goes to another machine first.
- `complete`, `fail` and `release` are idempotent for the same `lease_token` (repeat calls return 200).
  Any of them with a stale lease returns 409 and is discarded.
- **Idempotency key** of a match job: `sha256(engine "\n" ruleset_hash "\n" wasmtime "\n" map.sha256 "\n"
  bot_a.sha256 "\n" bot_a.fuel_mult "\n" bot_b.sha256 "\n" bot_b.fuel_mult "\n" seed "\n" max_ticks-or-empty)`,
  numbers in shortest decimal form. Submitting a job whose key already exists returns the existing job (and
  its result if done) instead of queueing a second one. Team order matters: A-vs-B and B-vs-A are different.
- **Priority classes**: `tournament` (0) > `ranked` (1) > `unranked` (2) > `bench` (3). `lease` hands out
  the lowest class number first, oldest job first within a class. No pre-emption of running jobs.
- **Aging**: a queued job is promoted one class for every **10 min** (suggested) it has waited, but never
  into `tournament`: tournament jobs always go first, while ranked, unranked and bench cannot starve one
  another behind a permanently busy higher class.

### Runner blobs (1.1, optional)

The organisers may upload an `ibc` binary of a patch as a blob and register it for that patch, per platform
(`"<os>-<arch>"` as Rust names them: `linux-x86_64`, `windows-x86_64`, `macos-aarch64`). Jobs of the patch
then carry `runner_blobs`. A worker that does not have the patch's runner, **and was started with an explicit
opt-in** (it means running native code the coordinator serves), downloads the blob for its platform, verifies
its hash, installs it side by side and checks that its own `version --json` is exactly the job's patch before
using it. Without the opt-in, or without a blob for its platform, the worker `release`s the job as a patch
mismatch. This lets a fleet move to a new patch without redeploying workers.

### Patch boundary

A job is pinned to the patch in its payload and is only ever run on that patch. When the organisers set a new
current patch (`POST /v1/admin/patch`), nothing is voided:

- **Started series finish on the old patch.** A series with a game running (leased) or finished keeps all
  its games on the patch it started on; workers that still have the old runner (or fetch it through
  `runner_blobs`) serve them. Tournament series always finish on their patch, started or not.
- **Series that have not started are re-planned** on the new patch: same games (maps, seeds, sides), new
  patch. The new jobs keep the old jobs' priority class and place in the queue; the old jobs are deleted.
  A re-planned game may hit the result cache.
- **Drain window.** Jobs of a superseded patch still *queued* `IBC_PATCH_DRAIN_SECS` (default 3600) after
  the switch are dead-lettered with `error: "patch superseded: ..."`; their series close with the games that
  were played (a series with none is void). Tournament jobs are never expired this way: keep a worker of the
  old patch until the tournament is over. `GET /v1/patch` reports `draining_jobs`.
- **Ratings.** Games are rated on the patch they were played on. With `{"balance": true}` the switch also
  widens every team's rating uncertainty by `IBC_BALANCE_SIGMA_FACTOR` (default 1.5,
  `RatingSystem::on_balance_patch`), recorded in the rating ledger as one `balance` event per team. Setting
  the patch that is already current changes nothing but its runner blobs and ABI.
- **Revalidation.** Every team's live submission is checked against the new patch (`imports` in the
  request lists the imports it offers bots, `{"ibc": [...], "wasi": [...]}`; default: the coordinator's own
  lists; the request body may be `ibc version --json` as printed, whose `abi` number is ignored).
  One that fails keeps playing for `IBC_INVALID_GRACE_SECS` (default 86400); after that its team is left out
  of ranked pairing until it makes a valid submission live. `GET /v1/team` shows `live_status` (`ok`,
  `grace`, `invalid`, or null without a live submission) with `invalid_reason`, `invalid_since` and
  `grace_until`; `GET /v1/submissions` shows each submission's `current_patch_check`; `GET /v1/patch` counts
  `teams_in_grace` and `teams_invalid`. A submission uploaded under an older patch is checked when it is made
  live, and refused (409 `invalid_on_patch`) if it fails.

The response of `POST /v1/admin/patch` says what happened: `switched`, `replanned_series`, `kept_series`
(running series still on an older patch), `ratings_widened`, `revalidated` and `invalid_submissions`.

### Languages and fuel multipliers

A submission's fuel multiplier is decided by the coordinator from its language table (`--language
name=mult`, default `rust=1`, `cpp=1`), never by the upload: `POST /v1/submissions?language=<name>` names
the language, an unknown one is a 400 `unknown_language` listing the accepted ones, and a `fuel_mult`
parameter is refused. The coordinator also reads what the module says about itself: an `ibc.language`
custom section (UTF-8 language name, for toolchains to embed), or else the `producers` section (`language`
Rust, C or C++; `processed-by` rustc or clang). If the module names languages and the declared one is not
among them, the upload is a 400 `language_mismatch`; without `?language=` a module naming exactly one
language is entered as that language, anything else as `rust`. This catches mistakes, not fraud: a module
can be stripped of both sections, and release builds often are.

### Spot verification

Results from untrusted workers are provisional. The coordinator re-queues a random sample of completed
jobs (suggested: 5 %, and 100 % of `tournament`) restricted to workers labelled `trusted`, and compares
`replay_sha256`. Equal: the result is confirmed. Different: the trusted result wins, the original worker is
flagged and its unverified results are re-queued; results of its that already counted are taken back (a
closed series is reopened, its rating changes are undone, and it is rated again from the re-runs). Anyone
holding a replay can also check it offline with
`ibc replay verify`, which re-simulates the inputs (it proves the replay is self-consistent, not that the
bots produced those inputs; only a re-run proves that).
