# Deploying workers

A worker is two binaries: `ibc` (the runner for one patch) and `ibc-worker` (a 3 MB supervisor that leases
jobs from the coordinator and feeds them to `ibc run-batch --jsonl`). It makes outbound HTTPS connections
only, keeps no state worth backing up, and can be killed at any time: a match is a pure function of
`(patch, map, bots, seed)`, so whatever it was doing is leased again by someone else. The protocol is
`docs/JOBS.md`.

| File | What it is | Status |
|---|---|---|
| `Dockerfile` | multi-stage build, distroless final image with `ibc` + `ibc-worker` only | **not built here** (no Docker daemon on the dev box) |
| `ibc-worker.service` | hardened systemd unit (DynamicUser, read-only root, private tmp, no new privileges) | **not run here** (written on Windows) |
| `cloud-init.yaml` | user data: boot a box into a registered worker; includes a spot interruption watcher | **not run here** |
| `docker-compose.yml` | coordinator + N workers on one box | **not run here**; the coordinator service is a placeholder |

The worker binary itself is tested (`cargo test -p ibc-worker`) and was run end to end against a mock
coordinator; see "Measured" below.

## Add a rented machine in five minutes

1. On the coordinator, issue a short-lived registration token.
2. Either paste `cloud-init.yaml` as user data with its two variables set (`IBC_COORDINATOR`,
   `IBC_REGISTRATION_TOKEN`), or on a box you already have:

   ```sh
   install -m 755 ibc ibc-worker /usr/local/bin/
   install -m 644 ibc-worker.service /etc/systemd/system/
   install -d -m 700 /etc/ibc-worker
   echo 'IBC_COORDINATOR=https://coordinator.example.org' > /etc/ibc-worker/env
   ( umask 077; printf '%s' "$TOKEN" > /etc/ibc-worker/registration-token )
   systemctl enable --now ibc-worker
   journalctl -u ibc-worker -f          # JSON lines: "registered", then "job leased" / "job completed"
   ```

   Or with Docker: `docker run -d -e IBC_COORDINATOR=... -e IBC_REGISTRATION_TOKEN=... ibc-worker:<patch>`.
3. The worker registers once and stores its own token in `worker-token.json` (mode 600). The registration
   token is not used again; empty the file. To remove the machine: `systemctl stop ibc-worker` (drains), then
   revoke its worker token on the coordinator.

Knobs (flags, or env in `/etc/ibc-worker/env`): `IBC_WORKER_CONCURRENCY` (default: all cores),
`IBC_WORKER_LABELS` (e.g. `trusted,x86_64-linux`), `IBC_WORKER_NAME`, `--cache-max-mb` (default 2048),
`--job-timeout-secs` (default 300). `ibc-worker --help` lists the rest.

### Building a release

```sh
cargo build --release -p ibc-cli -p ibc-worker
tar -czf ibc-worker-x86_64-linux.tar.gz -C target/release ibc ibc-worker -C ../../deploy ibc-worker.service
```

A worker serves the patch of its `--runner` (`ibc version --json`: engine, ruleset hash, wasmtime) and fails
jobs of any other patch with `patch mismatch`, retryable, so another worker takes them. Roll a new patch by
starting workers with the new binary, then draining the old ones. To serve several patches from one worker,
drop older runners at `<cache>/runners/<engine>-<ruleset_hash>-<wasmtime>/ibc`; each is used only after its
own `version --json` confirms the directory name. `--allow-runner-fetch` additionally lets a job name a
runner blob to install (off by default: it means running native code the coordinator serves).

## Bursting on spot instances

Workers are built for instances that vanish:

- **Drain** = stop leasing, finish and report what is running, exit 0. Triggers: `SIGTERM`/Ctrl-C, or the
  `--drain-file` appearing. A second signal, or `--drain-timeout-secs` (default 90) running out, kills the
  running matches and releases their jobs with `fail(retryable: true)`.
- **Interruption notices**: `cloud-init.yaml` installs `ibc-interruption-watch`, which polls the metadata
  service (AWS `spot/instance-action`, GCP `instance/preempted`, Azure scheduled events) and touches
  `/run/ibc-worker/drain`. AWS gives 120 s, GCP and Azure about 30 s; a match takes tens of milliseconds, so
  a drain normally loses nothing. On Kubernetes or ECS the orchestrator's SIGTERM does the same job.
- **No notice at all** (power loss, `kill -9`): nothing is reported, the leases expire after 180 s and the
  jobs return to the queue. That costs one of a job's three attempts, nothing else.
- **Scaling signal**: scale on the coordinator's `/metrics` queue signals, not on worker CPU (workers are
  either flat out or idle). Scale out when the queue depth divided by the fleet's measured completion rate
  exceeds the latency you accept (say, backlog > 2 minutes of work); scale in when lease polls come back
  empty. Start an auto-scaling group from `cloud-init.yaml` with a registration token that lives as long as
  the launch template; `tournament` jobs outrank `bench` jobs in the queue, so a burst fleet can be sized
  for the low-priority backlog alone.
- A long-polling idle worker costs one HTTP request per 25 s. An idle fleet is cheap for the coordinator
  but not for you: scale in to zero and keep only the trusted box.

## Security model

- **Workers see only compiled wasm and maps.** No source, no team identities (bot names in replays are hash
  prefixes), no credentials other than their own token. Bot code runs inside the runner's wasmtime sandbox
  with fuel limits; the worker never executes anything else it downloads (unless `--allow-runner-fetch`).
- **Everything is addressed by SHA-256.** A download is verified before it is given a name in the cache and
  again when it is taken out after sitting idle; a mismatch is discarded and the job failed as retryable.
- **Tokens are per worker and revocable.** The shared registration token is used once; after that each
  worker has its own bearer token, stored mode 600 and tied to one coordinator URL. Revoking it turns the
  worker's next call into a 401 and it stops. Use short-lived registration tokens for rented boxes.
- **Outbound only.** No listening socket; redirects are not followed (a token is never replayed to another
  host). Use `https://` for anything but localhost: the worker warns on plain `http://`.
- **Results from untrusted workers are provisional.** A worker reports the replay hash it computed itself
  from the file it uploads, but a malicious worker can still lie consistently. The coordinator therefore
  re-runs a sample (5 % suggested, 100 % of `tournament`) on workers labelled `trusted` and compares
  `replay_sha256`; a mismatch flags the worker and re-queues its unverified results. Only give the `trusted`
  label to machines you control.
- **The box is protected from the worker.** The systemd unit runs it as a dynamic user with a read-only
  root, private `/tmp`, no capabilities and no new privileges; the container runs as non-root on a
  read-only filesystem. The cache is capped; per-job scratch directories are deleted after each job and
  wiped on start. Give each worker process its own cache directory.

## Measured, and what it costs

Measured on the dev machine (20 logical cores, Windows 11, shared with ten other builds at the time, so
treat the parallel figures as a floor), prebuilt `ibc` 0.1.0, rusher vs holder on the two pool maps:

| Run | Wall | Throughput | Matches per core-second |
|---|---|---|---|
| `ibc run-batch --jsonl --jobs 1`, 40 matches | 1.13-1.26 s | 32-35 /s | **32-35** (one core) |
| `ibc run-batch --jsonl --jobs 8`, 40 matches | 0.57-0.58 s | 69-70 /s | **8.6-8.8** by wall x 8 (11 by summed per-match time) |
| one process per match, 10 matches | 3.0-3.1 s | 3.3 /s | 3.3 |
| `ibc-worker --concurrency 8` + mock coordinator over HTTP, 2000 matches (two runs: busy box, quieter box) | 22.0 s, 11.7 s | 91 /s, 172 /s | **11.4, 21.5** |
| `ibc-worker --concurrency 4`, 4000 matches incl. a drain and restart (same two runs) | 64 s, 37 s | 62 /s, 109 /s | 15.5, 27 |

What this says:

- A warm match costs about 25-30 ms of one core; a match in a fresh process costs about 300 ms. That is
  why the worker keeps runner processes alive (one per slot, so a timed-out match can be killed alone).
- Per-match cost roughly triples when eight run at once in one runner process (88 ms vs 26 ms summed
  per-match wall time), and the worker's figure doubled when the box got quieter. Parallel numbers here are
  noisy; measure on the target machine. The spread is the difference between 9 and 33 matches per
  core-second.
- The worker's own overhead (lease, three cached blobs, replay upload, complete) is not the bottleneck: it
  delivered more per core than the bare `--jobs 8` batch.

Sizing, using the conservative parallel figure (9 matches per core-second) and the single-core figure (33):

| | conservative | if parallel scaling is fixed |
|---|---|---|
| matches per core-hour | 32 000 | 119 000 |
| core-hours per 1 M matches | 31 | 8.4 |
| cost per 1 M matches at $0.01 / vCPU-hour (typical spot) | $0.31 | $0.08 |
| cost per 1 M matches at $0.04 / vCPU-hour (typical on-demand) | $1.25 | $0.34 |

The hourly prices are illustrative round numbers, not quotes; put in your own. These matches are between
light reference bots (about 10 M fuel per team); a fuel-hungry bot can cost up to the fuel cap, so re-run
the benchmark with real submissions before committing to a fleet size. Replays are about 18 KB each:
1 M matches is about 18 GB of blobs on the coordinator, which will cost more than the compute.

Reproduce: `ibc run-batch --jsonl --jobs 8 < jobs.jsonl` with 40 job lines for the runner alone;
`cargo run -p ibc-worker --release --example mock-coordinator -- --jobs 2000 --port 8787` and
`ibc-worker --coordinator http://127.0.0.1:8787 --registration-token mock --concurrency 8 --once` for the
whole path.
