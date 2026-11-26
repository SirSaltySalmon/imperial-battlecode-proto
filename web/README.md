# Web platform

The human-facing pages over the coordinator's `/v1` API: accounts, teams, submissions, the ladder, matches
with the embedded replay viewer, scrims, tournaments and the organisers' page. TypeScript built with Vite, no
UI framework and no runtime dependencies, like `viewer/`. Styling is a placeholder.

## Build and test

```
cd web
npm ci
npm test          # vitest: API client, router, formatting helpers
npm run build     # type-checks and writes dist/
```

## Served by the coordinator

`ibc-coordinator` serves the built files itself, so a deployment is one binary plus two folders:

| Path | Files | Setting |
|---|---|---|
| `/` | `web/dist` | `IBC_WEB_DIR` / `--web-dir` |
| `/viewer/` | `viewer/dist` (see `viewer/README.md`) | `IBC_VIEWER_DIR` / `--viewer-dir` |
| `/v1/...` | the API | |

Without a setting the coordinator looks for `web/dist` (or `web/`) beside its binary, then `web/dist` under the
working directory; the same for `viewer`. A folder counts only if it holds an `index.html`. With neither found
it serves the API alone and says so at start.

Rules of the static serving (`crates/coordinator/src/api/static_files.rs`):

- Paths are percent-decoded per segment; `.`, `..`, separators, drive prefixes and NUL are refused, so nothing
  outside the two folders is reachable.
- `assets/*` (Vite's content-hashed files) are cached for a year as immutable; everything else is `no-cache`.
- Content types by extension, including `application/wasm` for the viewer's engine build.
- A web path with no file extension that names no file (`/device`, `/teams/x`) gets `index.html`, because the
  pages route on the client. Missing files with an extension are 404, and the viewer has no such fallback.
- An unknown `/v1/...` path is the API's JSON 404 with `"error": "no_route"`, never a page. The client shows
  that as "not available yet", which is how pages for endpoints a server does not have yet degrade.

## Development

```
IBC_COORDINATOR=http://127.0.0.1:8080 npm run dev
```

The Vite dev server proxies `/v1` and `/viewer` to that coordinator.

## How it is put together

- `src/api.ts`: the typed client and the contract types. Cookie session (`credentials: "same-origin"`) with
  `X-IBC-CSRF: 1` on every write, or a bearer token pasted on the sign-in page (kept for the tab; for
  organisers and development). Errors become `ApiError` with the server's message, the retry time of a 429,
  and `unavailable` for endpoints the server does not have.
- `src/router.ts`: hash routes (`#/teams/:id`). The coordinator answers `index.html` for `/device?code=…`
  (the URL `ibc login` prints) and the page turns it into `#/device?code=…`.
- `src/session.ts`: who is signed in (`GET me`, or `GET team` for a team or organiser token).
- `src/pages/*.ts`: one module per area. Every section loads on its own and shows its own error, so one
  missing endpoint does not blank a page.
- Text goes into the page through `textContent` only; server-supplied links are limited to http(s) and
  same-origin URLs.

Matches link to the viewer as `/viewer/?replay=/v1/matches/<id>/replay` (same origin, so no CORS) and offer
the `.ibcr` for download.
