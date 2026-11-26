# API: accounts, sessions, teams, device login

Part of the coordinator's HTTP API (`crates/coordinator`, all paths under `/v1`). Errors are
`{"error": "<code>", "message": "..."}`; a 429 also carries `retry_after` (seconds) and a `Retry-After` header.
Code: `src/api/auth.rs`, `src/api/teams.rs`, `src/accounts.rs`, `src/db/users.rs`.

## Who is calling

Every team route (`GET team`, `submissions`, `series`, ...) accepts any of:

| Credential | Looks like | Where from |
|---|---|---|
| Team token, `Authorization: Bearer` | `ibt_<64 hex>` | `POST admin/teams` (organisers) or `POST team/token` |
| User token, `Authorization: Bearer` | `ibu_<64 hex>` | device login (`ibc login`) |
| Session cookie `ibc_session` (or the same value as a bearer token) | `ibs_<64 hex>` | `auth/register`, `auth/login` |
| The admin token, `Authorization: Bearer` | anything | `IBC_ADMIN_TOKEN` |

A user acts for their team; a user in no team gets 403 `no_team` on team routes. A bearer token wins over the
cookie, and a bad bearer token is a 401 even when a valid cookie is present. Users with `is_admin` pass every
admin check, like the admin token; when such a user is in no team, team routes treat them as the organisers
(`?team=` names the team). Banned users (`users.banned_at` set) get 403 `banned` everywhere.

**CSRF.** A write (any method but GET/HEAD/OPTIONS) authenticated by the cookie must carry the header
`X-IBC-CSRF: 1`, or it is refused with 403 `csrf`. A cross-site form or `<img>` cannot set a custom header, and
`SameSite=Lax` already keeps the cookie off cross-site POSTs; the header covers older browsers and same-site
subdomains. Bearer requests never need it.

The cookie is `ibc_session=<token>; HttpOnly; SameSite=Lax; Path=/; Max-Age=604800`, plus `Secure` when
`IBC_COOKIE_SECURE=1` (`true`/`yes` also work; set it whenever the site is served over HTTPS).

## Accounts

| Endpoint | Auth | Body | Answer |
|---|---|---|---|
| `POST auth/register` | none | `{email, password, display_name}` | 201 `{user, session_token}` + cookie. 409 `email_taken` |
| `POST auth/login` | none | `{email, password}` | 200 `{user, session_token}` + cookie. 401 `invalid_credentials`, 429 `rate_limited` |
| `POST auth/logout` | cookie, `ibs`/`ibu` bearer | none | 200 `{ok: true}`, clears the cookie; ends the session or revokes the `ibu` token used |
| `GET me` | user | | `{user: {id, email, display_name, is_admin}, team: {id, name, join_code, members: [{user_id, display_name, role}]} \| null}` |

- Emails are trimmed, lower-cased and unique. Passwords: 8 to 256 characters, stored as argon2id PHC strings
  (19 MiB, t=2, p=1). Display names: 1 to 40 printable characters.
- A wrong password and an unknown email give the same 401 and take the same time (a hash is computed either
  way).
- **Throttling:** 10 failed sign-ins for one email within 15 minutes and that email gets 429 `rate_limited`
  until the oldest failure leaves the window, even with the right password. An attempt is counted when it
  starts (in the same transaction as the check, so parallel guesses cannot slip past) and a success clears
  the count. It is keyed by email only, so a third party can keep one account locked out; per-IP limits
  belong in the reverse proxy.
- At most 4 password hashes run at once (each holds 19 MiB); more sign-ins wait their turn.
- A banned account gets 403 `banned` from sign-in whatever the password, so a ban is not a password oracle.
- Sessions last 7 days from sign-in. Emails listed in `IBC_ADMIN_EMAILS` become admins when they register or
  sign in. Emails are not verified, so **register the admin accounts before the site is public** (or set the
  variable only once they exist): whoever registers a listed email first becomes an admin.

## Device login (the CLI)

The shape of RFC 8628. The CLI asks for a code, shows `user_code` and `verify_url`, and polls; the person
approves the code in the browser, where they are signed in.

| Endpoint | Auth | Body | Answer |
|---|---|---|---|
| `POST auth/device` | none | none | `{device_code, user_code, verify_url, interval: 5, expires_in: 600}` |
| `POST auth/device/approve` | user (cookie + CSRF) | `{user_code}` | 200 `{ok, user_code}`; 404 if unknown, expired or already approved |
| `POST auth/device/token` | none | `{device_code}` | 200 `{token, user}` once approved (`token` is an `ibu_` user token) |

`auth/device/token` answers, until approval: 428 `authorization_pending`; 429 `slow_down` (with
`Retry-After: 5`) when polled again within the interval (up to one second early is tolerated); 400
`expired_token` after 10 minutes; 400 `invalid_grant` for an unknown or already used code. The token is issued
once: the code is deleted when it is handed out. `user_code` is `ABCD-EFGH` (consonants only); approval
accepts it in any case, with or without the dash. `verify_url` is `{IBC_PUBLIC_URL}/device?code=<user_code>`
(default `http://<listen address>`, `localhost` for a wildcard address); the web site serves that page.

## Teams

Teams have 1 to `IBC_TEAM_MAX` (4) members, and a user is in at most one team.

| Endpoint | Auth | Body | Answer |
|---|---|---|---|
| `POST teams` | user | `{name}` | 201 `{team, join_code}`; 409 `already_in_team`, `team_exists` |
| `POST teams/join` | user | `{join_code}` | 200 `{team}`; 404 unknown code, 409 `already_in_team`, `team_full` |
| `POST teams/leave` | user | none | 200 `{ok}`; 409 `no_team` |
| `POST teams/join-code/rotate` | team (any credential), admin with `?team=` | none | `{team_id, join_code}`; the old code stops working |
| `POST team/token` | team (any credential), admin with `?team=` | none | `{team_id, name, token}`: a new `ibt_` token, shown once; the old one stops working |
| `GET team` | team | | `{team_id, name, live_submission}` (unchanged) |

- Team names are 1 to 40 characters of `[A-Za-z0-9_.-]`, the same rule as `POST admin/teams`. The creator's
  role is `owner`, others are `member`; roles do not gate anything yet.
- Creating a team does not show its `ibt` token; members who want one for automation call `POST team/token`.
- A team keeps its submissions and rating when members leave, even when none are left. Leaving replaces the
  team token, so a copy the leaver took stops working; members regenerate it with `POST team/token`.
- One account can create at most 3 teams over its lifetime (403 `team_limit`), since each team has its own
  unranked quota.
- Teams created by the organisers (`POST admin/teams`) get a join code from `teams/join-code/rotate`, called
  with the team token or by an admin with `?team=`; hand it to the people who should join.

## Storage

Tables `users`, `sessions`, `user_tokens`, `device_codes`, `team_members`, `login_failures`, and the column
`teams.join_code`. Session tokens, user tokens and device codes are stored as SHA-256 only.
