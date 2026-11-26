# Moderation API

All routes need the organiser token (`Authorization: Bearer $IBC_ADMIN_TOKEN`). Errors use the usual
`{"error", "message"}` shape. `ibc admin` (see `docs/MODERATION.md`) is a client of these routes.

## Sanctions

| Route | Body | Answer |
|---|---|---|
| `POST /v1/admin/teams/{team}/suspend` | `{reason, until?}` | `{team_id, name, kind:"suspended", reason, until}` |
| `POST /v1/admin/teams/{team}/ban` | `{reason}` | `{team_id, name, kind:"banned", reason, until:null}` |
| `POST /v1/admin/teams/{team}/unban` | `{reason?}` (audit log only) | `{team_id, name, lifted}` |
| `GET /v1/admin/sanctions` | | `{sanctions:[{team_id, name, kind, reason, until, since, active}]}` |

`{team}` is a team id or name. `reason` is required (1..=500 characters) and is shown to the team. `until`
(suspensions only) is `7d`, `12h`, `30m`, `2026-12-01` or `2026-12-01T18:00:00Z` (UTC); without it the
suspension lasts until it is lifted. A ban has no `until` (400 if given). A new sanction replaces the old one.

While a sanction is in force the team:

- is left out of ranked pairing (`matchmaking_pool`);
- cannot be in any new series (ranked pairings, scrims by it or against it, tournament series): 403 for the
  team itself (or the organisers); a team asking for a scrim against it gets 409 `opponent_unavailable`
  without the reason;
- cannot upload, and every write it makes with its team token answers 403. Reads still work.

```json
HTTP 403
{"error": "team_suspended", "message": "team \"alpha\" is suspended until 2026-12-01T00:00:00Z: shared a token",
 "kind": "suspended", "reason": "shared a token", "until": "2026-12-01T00:00:00Z"}
```

Series already running when the sanction starts finish normally.

## Overview

`GET /v1/admin/overview`

```json
{"queue": [{"class": "ranked", "state": "queued", "jobs": 3, "oldest_age_secs": 12}],
 "workers": {"live": 2, "live_cores": 8, "quarantined": 0, "total": 3},
 "games": {"last_hour": 40, "last_day": 900},
 "mean_core_seconds_per_game": 0.04,
 "dead_letter": 0,
 "teams": {"total": 12, "with_live_submission": 11, "sanctioned": 1},
 "uploads_last_hour": 5,
 "pause": {"ranked_scheduler": "every 300 s"},
 "limits": {"uploads_per_hour": 20, "api_per_minute": 120, "public_per_minute": 300,
            "submissions_kept": 200, "max_wasm_bytes": 16777216}}
```

`games` counts finished jobs. `mean_core_seconds_per_game` is the mean `wall_ms` of the last hour (else the last
day; `null` without data): a worker slot runs one match on one core. `pause` lists every `settings` key containing
`pause` plus the ranked scheduler interval.

## Flags

`GET /v1/admin/flags` → `{flags:[{kind, team, team_name, worker, detail, first_seen}]}`, computed on each call.

| `kind` | Raised when |
|---|---|
| `upload_burst` | a team made more than `IBC_UPLOAD_BURST_FLAG` (10) uploads in the last hour |
| `duplicate_wasm` | the same validated module (sha256) was uploaded by more than one team; one flag per team |
| `rejected_upload` | a team's newest upload failed validation (it plays an older module, or none) |
| `failing_live_submission` | games of a team's live submission were dead-lettered in the last 24 h |
| `worker_quarantined` | a worker is quarantined (`detail` has the reason) |

`first_seen` is when the data says the condition started, else when a call first saw it. A flag that no longer
holds is forgotten.

## Audit log

`GET /v1/admin/audit?limit=100&before=SEQ` → `{entries:[{seq, at, actor, action, status, payload}]}`, newest
first. Every write (`POST`, `PUT`, `DELETE`, ...) made with the organiser token on any route is recorded with the response status: `action` is `"<METHOD> <path?query>"`, `payload` the JSON body
(bodies over 16 KiB or not JSON are recorded as `{"bytes": n}`). The table refuses `UPDATE` and `DELETE`.

## Rate limits

Every route except the authenticated worker protocol (`/v1/workers/*` but not `register`, `/v1/jobs/*`,
`/v1/blobs/*`, `/healthz`) passes the rate limiter. The organiser token is never limited.

| Limit | Key | Default | Variable |
|---|---|---|---|
| Requests with a team token | the token | 120 / minute | `IBC_API_PER_MINUTE` |
| Any other request (anonymous, unknown token, worker registration) | client IP | 300 / minute | `IBC_PUBLIC_PER_MINUTE` |
| Uploads (`POST /v1/submissions`) | team, rolling hour | 20 / hour | `IBC_UPLOADS_PER_HOUR` |

A made-up token therefore spends its IP's budget. User tokens and session cookies (sign-in) count per IP until
`credential_team` in `api/moderation.rs` learns to resolve them.

Over a limit:

```json
HTTP 429
Retry-After: 17
{"error": "rate_limited", "message": "rate limit: 120 requests per minute per token; retry after 17 s", "retry_after": 17}
```

The request limits are token buckets in memory (a restart refills them). The upload limit counts the team's
rows in `submissions` of the last hour, rejected uploads included (retention never prunes those), so it
survives restarts; it is checked again under the database lock, so parallel uploads cannot overshoot it. A module over `IBC_MAX_WASM_BYTES`
answers 413 `too_large` with its size and the limit, before the body is read when `Content-Length` is sent.
