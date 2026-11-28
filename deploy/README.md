# Deploying workers

A worker is two binaries: `ibc` (the runner for one patch) and `ibc-worker` (a 3 MB supervisor that leases
jobs from the coordinator and feeds them to `ibc run-batch --jsonl`). It makes outbound HTTPS connections
only, keeps no state worth backing up, and can be killed at any time: a match is a pure function of
`(patch, map, bots, seed)`, so whatever it was doing is leased again by someone else. The protocol is
`docs/JOBS.md`.

Running the coordinator, backups, replay retention and publishing releases to participants are in
`docs/DEPLOY.md`; this page is about workers.

| File | What it is | Status |
|---|---|---|
| `Dockerfile` | multi-stage build; targets `worker` (`ibc` + `ibc-worker`) and `coordinator` (`ibc-coordinator` + viewer + web front end), distroless | **not built here** (no Docker daemon on the dev box) |
| `docker-compose.yml` | coordinator + two trusted workers on one box, ranked scheduler on | `docker compose config` validates; **not run here** |
| `ibc-worker.service`, `ibc-coordinator.service` | hardened systemd units (DynamicUser, read-only root, private tmp, no new privileges); shipped in the Linux release archive | **not run here** (written on Windows) |
| `cloud-init.yaml` | user data: boot a box into a registered worker from the Linux release archive; includes a spot interruption watcher | **not run here** |
| `backup.sh` | `ibc-coordinator prune-replays` + `backup` + dated snapshots, for cron | the two commands are tested (`cargo test -p ibc-coordinator --test operations`); the script is not run here |

The worker binary itself is tested (`cargo test -p ibc-worker`) and was run end to end against a mock
coordinator; see "Measured" below. The same coordinator and workers, natively, are what
`tools/devstack.sh` runs: that is the tested path.

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
cargo xtask dist --target x86_64-unknown-linux-musl     # dist/ibc-<version>-x86_64-unknown-linux-musl.tar.gz
```

The archive holds `ibc`, `ibc-worker` and `ibc-worker.service` (and the coordinator, viewer, maps and docs)
in one top-level directory; `cloud-init.yaml` unpacks it with `--strip-components=1`. The tagged release
builds it for six targets (`docs/release.md`).

A worker serves the patch of its `--runner` (`ibc version --json`: engine, ruleset hash, wasmtime) and
`release`s jobs of any other patch ("patch mismatch"): the job goes back to the queue without spending an
attempt, and the coordinator keeps it away from that worker for its retry grace, so another worker takes it.
Roll a new patch by starting workers with the new binary, then draining the old ones. To serve several
patches from one worker, drop older runners at `<cache>/runners/<engine>-<ruleset_hash>-<wasmtime>/ibc`;
each is used only after its own `version --json` confirms the directory name. `--allow-runner-fetch`
additionally installs the runner blob a job names for its platform (`runner_blobs`, registered with the
patch on the coordinator; off by default: it means running native code the coordinator serves).

## Bursting on spot instances

Workers are built for instances that vanish:

- **Drain** = stop leasing, finish and report what is running, exit 0. Triggers: `SIGTERM`/Ctrl-C, or the
  `--drain-file` appearing. A second signal, or `--drain-timeout-secs` (default 90) running out, kills the
  running matches and gives their jobs back with `release` (no attempt spent). A lease long poll that is
  open when the drain begins is not cut short: an idle worker exits when it returns (at most `--poll-ms`,
  25 s), and any job it brings back is released unstarted.
- **Interruption notices**: `cloud-init.yaml` installs `ibc-interruption-watch`, which polls the metadata
  service (AWS `spot/instance-action`, GCP `instance/preempted`, Azure scheduled events) and touches
  `/run/ibc-worker/drain`. AWS gives 120 s, GCP and Azure about 30 s; a match takes tens of milliseconds, so
  a drain normally loses nothing. On Kubernetes or ECS the orchestrator's SIGTERM does the same job.
- **No notice at all** (power loss, `kill -9`): nothing is reported, the leases expire after 180 s (the
  coordinator's `--lease-secs`) and the jobs return to the queue. That costs one of a job's three attempts,
  nothing else. Measured against the real coordinator: `cargo test -p ibc-worker --test e2e` kills a worker
  holding leases and checks that the other worker finishes its jobs.
- **Timeouts**: a match past `--job-timeout-secs` is killed and failed with `code: "timeout"`; the
  coordinator retries it once elsewhere and dead-letters it on a second timeout (matches are deterministic).
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

Measured on the dev machine on 2026-11-27 (i7-12700H laptop: 6 two-thread performance cores and 8
efficiency cores, Windows 11), `ibc` 0.2.0, 200 rusher-vs-holder matches on `crossfire` and `quarry` kept N
in flight through `ibc run-batch --jsonl`, at high priority because the box was shared with other builds; 3
repeats, ranges in brackets. These are from just before the runner stopped deflating each replay twice
(`docs/TODO.md`, section 2), which made every row about 20% cheaper (16 to 34%, paired interleaved runs).

| Matches at once | One process, `--jobs N`: ms per match | Matches per core-second | N processes, `--jobs 1` each (the worker): ms per match | Matches per core-second |
|---|---|---|---|---|
| 1 | 54 [49, 63] | 18 | | |
| 2 | 53 [51, 55] | 19 | 52 [50, 55] | 19 |
| 4 | 54 [52, 57] | 18 | 56 [51, 64] | 18 |
| 8 | 66 [64, 70] | 15 | 66 [61, 74] | 15 |
| 16 | 89 [88, 90] | 10 | 80 [79, 83] | 11 |

Earlier, with `ibc` 0.1.0 on the same box shared with ten other builds: one process per match managed 3.3
matches/s, and `ibc-worker` with the mock coordinator over HTTP ran 2000 matches at `--concurrency 8` at
11.4 and 21.5 matches per core-second (busy box, quieter box), and 4000 at `--concurrency 4`, a drain and
restart included, at 15.5 and 27.

What this says:

- A warm match costs about 55 ms of one core here (about 45 since the change); a match in a fresh process
  cost about 300 ms with 0.1.0 (Wasmtime start-up and compiling both bots). That is why the worker keeps
  runner processes alive (one per slot, so a timed-out match can be killed alone).
- Matches side by side cost what the CPU's shared cores cost: separate processes slow down just as much.
  One runner process adds nothing up to 8 at once and about 11% at 16 (Wasmtime maps and unmaps each unit's
  memory and stack, which serialises within a process). Parallel numbers are noisy; measure on the target
  machine.
- The worker's own overhead (lease, three cached blobs, replay upload, complete) is not the bottleneck: it
  delivered more per core than the bare batch.

Sizing from the table, before the change (so about 20% pessimistic):

| | 16 at once (11 per core-second) | one at a time (18 per core-second) |
|---|---|---|
| matches per core-hour | 40 000 | 65 000 |
| core-hours per 1 M matches | 25 | 15 |
| cost per 1 M matches at $0.01 / vCPU-hour (typical spot) | $0.25 | $0.15 |
| cost per 1 M matches at $0.04 / vCPU-hour (typical on-demand) | $1.00 | $0.62 |

The hourly prices are illustrative round numbers, not quotes; put in your own. These matches are between
light reference bots (about 13 M fuel per team); a fuel-hungry bot can cost up to the fuel cap, so re-run
the benchmark with real submissions before committing to a fleet size. Replays are about 46 KB each:
1 M matches is about 46 GB of blobs on the coordinator, which will cost more than the compute.

Reproduce: `ibc run-batch --jsonl --jobs 8 < jobs.jsonl` with job lines for the runner alone;
`cargo run -p ibc-worker --release --example mock-coordinator -- --jobs 2000 --port 8787` and
`ibc-worker --coordinator http://127.0.0.1:8787 --registration-token mock --concurrency 8 --once` for the
whole path.
