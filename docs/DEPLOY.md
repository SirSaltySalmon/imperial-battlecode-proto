# Running the platform

The whole online side is two binaries: `ibc-coordinator` (API, job queue, ranked scheduler, ladder; one
process, one data directory) and `ibc-worker` (leases matches and plays them with `ibc`). Participants use
one more, `ibc`, from a release archive or `pip install imperial-battlecode`. This page is the operator's
guide: try it locally, put it on a server, publish releases to participants, back it up.

Worker details (spot instances, the security model, measured throughput) are in `deploy/README.md`; the
protocol between the two is `docs/JOBS.md`; releasing a patch is `docs/release.md`.

## Try it on one machine

```sh
tools/devstack.sh                        # port 18016; keeps running until Ctrl-C
tools/devstack.sh 18016 --rounds 3 --exit
```

`devstack.sh` builds the three binaries (debug), starts a coordinator and two workers, loads the patch of the
built `ibc` and the map pool, builds the eight reference bots with `ibc build`, makes each a team with a live
submission, and turns the ranked scheduler on (a round every 30 s). It waits until every team has played
`--rounds` ranked series, prints the ladder and the latest matches, and then either exits or keeps the
stack up for you to point `ibc`, a browser or the web front end at (`IBC_SERVER=http://127.0.0.1:18016`).
Everything lives in `out/devstack/` (data directory, worker caches, logs) and is wiped at the next start.
It needs bash (Git Bash on Windows), curl, and the `wasm32-wasip1` Rust target.

Both devstack workers register with the trusted registration token, so the stack runs unattended. With only
untrusted workers it would not: a share of their results (`IBC_VERIFY_FRACTION`, default 0.05, and every
tournament job) is re-run by a trusted worker before the series closes, and with no trusted worker those
series wait. On a real deployment keep at least one trusted worker running, or set `IBC_VERIFY_FRACTION=0`
if every worker is trusted.

## On a server

Pick one of the two. Both run the coordinator behind a TLS-terminating proxy (Caddy or nginx) and keep all
state in one directory.

**Docker Compose** (one box, coordinator + two trusted workers):

```sh
export IBC_ADMIN_TOKEN=$(openssl rand -hex 24)
export IBC_REGISTRATION_TOKEN=$(openssl rand -hex 24)          # for workers on other machines
export IBC_TRUSTED_REGISTRATION_TOKEN=$(openssl rand -hex 24)  # for the two workers in the file
docker compose -f deploy/docker-compose.yml up -d --build
```

`deploy/Dockerfile` has two targets: `worker` (`ibc` + `ibc-worker`) and `coordinator` (`ibc-coordinator`
plus the built replay viewer and, if the checkout has `web/`, the web front end, at `/srv/viewer` and
`/srv/web`, announced to the coordinator through `IBC_VIEWER_DIR` and `IBC_WEB_DIR`; the coordinator in
this tree does not serve static files yet and ignores both). The coordinator's data
is the `coordinator-data` volume, mounted at `/data`.

**systemd** (no containers): unpack the Linux release archive and install the units it contains.

```sh
tar -xzf ibc-<version>-x86_64-unknown-linux-musl.tar.gz && cd ibc-<version>-x86_64-unknown-linux-musl
install -m 755 ibc ibc-worker ibc-coordinator /usr/local/bin/
install -m 644 ibc-coordinator.service ibc-worker.service /etc/systemd/system/
# /etc/ibc-coordinator/env: IBC_ADMIN_TOKEN=..., the registration tokens, IBC_RANKED_INTERVAL_SECS=300
systemctl enable --now ibc-coordinator
```

Workers on the same box register with the trusted token (`deploy/README.md`); rented machines boot from
`deploy/cloud-init.yaml` with the untrusted one.

### Configuration

Every flag has an `IBC_*` variable (`ibc-coordinator --help`). The ones a deployment sets:

| Variable | Meaning |
|---|---|
| `IBC_ADMIN_TOKEN` | the organisers' bearer token (required) |
| `IBC_DATA` | data directory: `coordinator.sqlite` and `blobs/` |
| `IBC_LISTEN` | `127.0.0.1:8080` by default; `0.0.0.0:8080` in the container |
| `IBC_REGISTRATION_TOKEN`, `IBC_TRUSTED_REGISTRATION_TOKEN` | what workers register with; the trusted one gives final results and runs spot verification |
| `IBC_RANKED_INTERVAL_SECS` | seconds between ranked rounds; 0 (default) = off, rounds only by `POST /v1/admin/ranked-batch` |
| `IBC_RANKED_GAMES` | games per ranked series (keep it even: sides alternate) |
| `IBC_DISTRIBUTION` | `distribution.json` served at `GET /v1/distribution` (below) |
| `IBC_WEB_DIR`, `IBC_VIEWER_DIR` | built static sites, served by the coordinator when it supports them |

### First start

The coordinator starts empty. As the organisers (`-H "Authorization: Bearer $IBC_ADMIN_TOKEN"`):

```sh
ibc version --json                                      # the patch the workers' ibc plays
curl -X POST $S/v1/admin/patch -d '{"engine":"0.1.0","ruleset_hash":"...","wasmtime":"..."}'
for m in maps/*.ibcmap; do curl -X PUT --data-binary @$m $S/v1/admin/maps/$(basename $m .ibcmap); done
curl -X POST $S/v1/admin/teams -d '{"name":"alpha"}'    # until sign-up replaces it
```

`tools/devstack.sh` does exactly this; read it for the full sequence.

## Publishing a release to participants

A participant's `ibc` asks the server once a day what it runs (`GET /v1/distribution`) and warns when it is
older; `ibc update` prints the exact upgrade command (`pip install --upgrade "imperial-battlecode==X"`, or
the archive URL and its SHA-256 for their platform). Nothing is ever downloaded automatically.

1. The release workflow (`docs/release.md`) attaches `distribution.json` to the GitHub Release: every
   archive with its download URL and SHA-256. Locally, `cargo xtask dist --target <triple>` writes it next to
   the archives (`dist/distribution.json`; `--base-url` changes where the URLs point).
2. Put it on the server and point `IBC_DISTRIBUTION` at it: with Compose, `deploy/release/distribution.json`
   and `IBC_DISTRIBUTION=/release/distribution.json`; with systemd, a file the service can read, for example
   `/var/lib/ibc-coordinator/distribution.json` (`/etc/ibc-coordinator` is root-only).
3. The file is read on every request: replacing it publishes the new release, no restart needed.

`GET /v1/distribution` answers 404 with an explanation while `IBC_DISTRIBUTION` is unset. A workspace can
also pin the engine for everyone who checks it out: `ibc.toml` with `[engine] version = "x.y.z"`.

## Backups

Everything is in the data directory: `coordinator.sqlite` (teams, submissions, jobs, results, ratings) and
`blobs/` (bot modules, maps, replays, addressed by SHA-256). Workers keep nothing worth saving.

```sh
ibc-coordinator backup /var/backups/ibc-coordinator     # IBC_DATA must point at the data directory
deploy/backup.sh                                        # the same, plus retention and dated snapshots
```

`backup DIR` is safe while the coordinator runs: the database is snapshotted with SQLite's `VACUUM INTO`
(consistent, compact, no lock held for long) to `DIR/coordinator.sqlite`, then blobs are copied to
`DIR/blobs/`. Blobs never change, so only new ones are copied and an hourly backup into the same directory
is cheap. `deploy/backup.sh` also keeps the last `KEEP_SNAPSHOTS` database snapshots in
`DIR/snapshots/` and prunes old replays first (below); run it from cron or a systemd timer, then ship `DIR`
off the box with restic, rclone or rsync. With Compose: `COMPOSE="docker compose -f
deploy/docker-compose.yml exec -T coordinator" BACKUP_DIR=/data/backup deploy/backup.sh`, and copy the
volume's `backup/` out; in this mode only the latest database snapshot is kept (the image has no shell
to make dated copies), so keep dated copies on the side you copy it to.

**Restore:** stop the coordinator, copy `DIR/coordinator.sqlite` (or a snapshot, renamed) and `DIR/blobs/`
into an empty data directory, delete any `coordinator.sqlite-wal`/`-shm` left from before, start it.
Jobs that were leased at backup time are leased again once their leases expire.

## Replay retention

Replays are most of the disk (about 18 KB a match; a million matches is 18 GB) and all of them can be
regenerated: a match is a pure function of (patch, map, bots, seed), which the database keeps.

```sh
ibc-coordinator prune-replays --older-than 14 --kinds unranked [--dry-run]
```

deletes the replay blobs of finished matches of those kinds older than the given number of days. Results,
ratings and match lists stay; downloading a pruned replay answers 404. A replay shared with a match that
is kept (another kind, or a verification run of a kept match) is kept. Ranked and tournament replays are
kept unless you name them. To regenerate a pruned replay, run the match again with `ibc run` on the same
patch: the bots' modules and the map are still blobs, and the seed is in `GET /v1/matches/{id}`.
