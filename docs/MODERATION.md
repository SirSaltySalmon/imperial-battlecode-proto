# Moderation

How organisers watch the platform and act on teams. The HTTP routes are in `docs/api/moderation.md`.

## The terminal: `ibc admin`

```
ibc admin --url https://coordinator.example.org --token $IBC_ADMIN_TOKEN overview
```

`--url` and `--token` default to `IBC_COORDINATOR` and `IBC_ADMIN_TOKEN`. Every command prints a compact table;
`--json` prints the coordinator's JSON instead.

| Command | Shows or does |
|---|---|
| `overview` | queue by class and state, live workers and cores, games in the last hour and day, core-seconds per game, dead letters, pause switches, limits |
| `flags` | computed flags: upload bursts, one module uploaded by several teams, rejected newest upload, failing live submission, quarantined workers |
| `audit [--limit N] [--before SEQ]` | the append-only log of organiser writes |
| `teams` | every team, its live module and its sanction |
| `suspend TEAM --reason R [--until 7d]` | suspend a team, for a time or until lifted |
| `ban TEAM --reason R` | ban a team until `unban` |
| `unban TEAM [--reason R]` | lift a suspension or ban |
| `workers` | workers: cores, live, trusted, quarantined, leased jobs |
| `dead` / `requeue JOB` | the dead-letter list, and giving a dead job another round |

Quarantining a worker stays `POST /v1/admin/workers/{id}/quarantine` (see `docs/JOBS.md`).

## Sanctions

A suspended or banned team is out of ranked pairing, cannot be in a new series of any kind (ranked, scrim,
tournament; other teams asking for a scrim against it only see `opponent_unavailable`), and its uploads and other writes answer 403 `team_suspended` with the reason and the end date, so
the team (or its AI agent) sees why. It can still read its data. A suspension ends on its own at `until`; a ban
lasts until `unban`. Series already running finish.

Freezing a rating by withdrawing every bot is not possible: a validated submission stays live until another
replaces it, so the team keeps being paired.

Not done yet: kicking or banning a single user. Accounts and team membership arrive with the sign-in work
(`users`, team members); a user-level ban then belongs next to the team sanction in `db/moderation.rs`.

## Abuse limits

| What | Default | Variable |
|---|---|---|
| Uploads per team per rolling hour | 20 | `IBC_UPLOADS_PER_HOUR` (0 = off) |
| Requests per team token per minute | 120 | `IBC_API_PER_MINUTE` (0 = off) |
| Requests per client IP per minute without a team token | 300 | `IBC_PUBLIC_PER_MINUTE` (0 = off) |
| Client IP from the last `X-Forwarded-For` entry | off | `IBC_TRUST_PROXY=true` behind exactly one proxy |
| Largest bot module | 16 MiB | `IBC_MAX_WASM_BYTES` |
| Submissions kept per team | 200 | `IBC_SUBMISSIONS_KEPT` (0 = keep all) |
| Upload burst flag | more than 10 per hour | `IBC_UPLOAD_BURST_FLAG` |

Over a limit the answer is 429 `rate_limited` with `retry_after` and a `Retry-After` header. Authenticated
worker routes and the organiser token are never limited. A request with an unknown token, and worker
registration, spend their IP's budget, so guessing tokens does not get around the per-IP limit.

## Retention

After each upload the team's submissions beyond the newest `IBC_SUBMISSIONS_KEPT` are deleted, except:

- the live submission;
- submissions of the last hour (the upload limit counts them);
- any submission a series used (ranked, scrim or tournament: `series.sub_a` / `sub_b`), since match pages and
  replays name it. Tables added later that refer to submissions go in `PINS` in `db/moderation.rs`.

The module blob of a deleted validated submission is deleted too unless another submission (of any team), a
map, a runner binary or a queued, leased or dead-lettered job still refers to it (a rejected upload never
stored a blob). Replays are not touched: they hold the whole match and
re-verify without the bot modules.

## Backups

Everything is in the data directory: `coordinator.sqlite` (WAL) and `blobs/`. Copy the database with
`sqlite3 coordinator.sqlite ".backup backup.sqlite"` (safe while the coordinator runs), then copy `blobs/`
(content-addressed, so an incremental `rsync` is enough).
