import { describe, expect, it } from "vitest";
import { Api, ApiError, buildUrl, listOf, normalizeLadder, normalizeMatches, parseRetryAfter, toApiError } from "../src/api";
import { loadSession } from "../src/session";

interface Call {
  url: string;
  init: RequestInit;
}

/** A fetch that records calls and answers from a table of `METHOD path` -> [status, body, headers]. */
function fakeFetch(answers: Record<string, [number, unknown, Record<string, string>?]>) {
  const calls: Call[] = [];
  const f = async (input: RequestInfo | URL, init: RequestInit = {}) => {
    const url = String(input);
    calls.push({ url, init });
    const key = `${init.method ?? "GET"} ${url.split("?")[0]}`;
    const [status, body, headers] = answers[key] ?? [404, ""];
    const text = typeof body === "string" ? body : JSON.stringify(body);
    return new Response(text, { status, headers });
  };
  return { fetch: f as typeof fetch, calls };
}

describe("errors", () => {
  it("reads the API's error body", () => {
    const e = toApiError(409, null, '{"error":"conflict","message":"name taken"}');
    expect(e.status).toBe(409);
    expect(e.code).toBe("conflict");
    expect(e.message).toBe("name taken");
    expect(e.unavailable).toBe(false);
  });

  it("treats a bare 404 or no_route as an endpoint that is not there yet", () => {
    expect(toApiError(404, null, "").unavailable).toBe(true);
    expect(toApiError(404, null, '{"error":"no_route","message":"no such endpoint"}').unavailable).toBe(true);
    expect(toApiError(405, null, "").unavailable).toBe(true);
    expect(toApiError(404, null, '{"error":"not_found","message":"no match \\"x\\""}').unavailable).toBe(false);
  });

  it("takes the retry time from the header, else the body, for 429 only", () => {
    expect(toApiError(429, "30", '{"error":"rate_limited","message":"slow down"}').retryAfter).toBe(30);
    expect(toApiError(429, null, '{"error":"rate_limited","message":"x","retry_after":12.2}').retryAfter).toBe(13);
    expect(toApiError(400, "30", "").retryAfter).toBeNull();
  });

  it("parses Retry-After as seconds or a date", () => {
    expect(parseRetryAfter("5")).toBe(5);
    expect(parseRetryAfter(null)).toBeNull();
    const now = Date.parse("2026-01-01T00:00:00Z");
    expect(parseRetryAfter("Thu, 01 Jan 2026 00:01:00 GMT", now)).toBe(60);
    expect(parseRetryAfter("nonsense")).toBeNull();
  });
});

describe("client", () => {
  it("builds URLs without empty query values", () => {
    expect(buildUrl("/v1", "matches", { team: "t1", kind: undefined, limit: 20, before: "" })).toBe(
      "/v1/matches?team=t1&limit=20",
    );
    expect(buildUrl("/v1", "/me")).toBe("/v1/me");
  });

  it("sends the CSRF header on writes only, with same-origin credentials", async () => {
    const { fetch, calls } = fakeFetch({
      "GET /v1/me": [200, { user: null, team: null }],
      "POST /v1/teams": [201, { team: { id: "t1", name: "x" }, join_code: "J" }],
    });
    const api = new Api({ fetch });
    await api.me();
    await api.createTeam("x");
    const get = calls[0]!.init.headers as Record<string, string>;
    const post = calls[1]!.init.headers as Record<string, string>;
    expect(get["X-IBC-CSRF"]).toBeUndefined();
    expect(post["X-IBC-CSRF"]).toBe("1");
    expect(post["Content-Type"]).toBe("application/json");
    expect(calls[1]!.init.body).toBe('{"name":"x"}');
    expect(calls.every((c) => c.init.credentials === "same-origin")).toBe(true);
  });

  it("uploads the raw module with language, live and note in the query", async () => {
    const { fetch, calls } = fakeFetch({ "POST /v1/submissions": [201, { submission_id: "s1", live: true }] });
    const api = new Api({ fetch, token: () => "ibt123" });
    const wasm = new Blob([new Uint8Array([0, 97, 115, 109])]);
    await api.upload(wasm, "cpp", false, "  faster  ");
    expect(calls[0]!.url).toBe("/v1/submissions?language=cpp&live=false&note=faster");
    const headers = calls[0]!.init.headers as Record<string, string>;
    expect(headers["Content-Type"]).toBe("application/wasm");
    expect(headers.Authorization).toBe("Bearer ibt123");
    expect(calls[0]!.init.body).toBe(wasm);
  });

  it("throws ApiError with the server's message", async () => {
    const { fetch } = fakeFetch({
      "POST /v1/scrims": [429, { error: "quota_exceeded", message: "100 of 100 used" }, { "Retry-After": "600" }],
    });
    const err = await new Api({ fetch }).scrim({ opponent_team_id: "t2", games: 1 }).catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err.status).toBe(429);
    expect(err.message).toBe("100 of 100 used");
    expect(err.retryAfter).toBe(600);
  });
});

describe("normalisers", () => {
  it("reads the contract ladder and the older snapshot", () => {
    const contract = normalizeLadder([
      { rank: 1, team_id: "a", team_name: "Alpha", rating: 1612, tier: "Gold", provisional: false, games: 30 },
    ]);
    expect(contract[0]).toMatchObject({ team_name: "Alpha", tier: "Gold", rating: 1612, games: 30 });
    const old = normalizeLadder({
      teams: [{ rank: 1, team_id: "b", name: "Beta", rating: 1500, provisional: true, games: 0, wins: 0, losses: 0, draws: 0 }],
    });
    expect(old[0]).toMatchObject({ team_name: "Beta", tier: null, provisional: true, wins: 0 });
    expect(normalizeLadder(null)).toEqual([]);
  });

  it("reads match pages and bare lists", () => {
    expect(normalizeMatches({ matches: [{ match_id: "m" }], next: "m" }).next).toBe("m");
    expect(normalizeMatches([{ match_id: "m" }]).matches).toHaveLength(1);
  });

  it("finds the list in a wrapped response", () => {
    expect(listOf([1], "x")).toEqual([1]);
    expect(listOf({ x: [2] }, "x")).toEqual([2]);
    expect(listOf({ total: 3, rows: [4] }, "x")).toEqual([4]);
    expect(listOf({}, "x")).toEqual([]);
  });
});

describe("session", () => {
  it("uses GET me for a session", async () => {
    const { fetch } = fakeFetch({
      "GET /v1/me": [200, { user: { id: "u", email: "a@b", display_name: "A", is_admin: true }, team: null }],
    });
    const s = await loadSession(new Api({ fetch }));
    expect(s).toMatchObject({ via: "session", isAdmin: true, team: null });
  });

  it("is signed out when me says 401 and there is no token", async () => {
    const { fetch } = fakeFetch({ "GET /v1/me": [401, { error: "unauthorized", message: "no" }] });
    expect((await loadSession(new Api({ fetch }))).via).toBe("none");
  });

  it("falls back to GET team for a team or organiser token", async () => {
    const team = fakeFetch({ "GET /v1/team": [200, { team_id: "t1", name: "Alpha", live_submission: null }] });
    expect(await loadSession(new Api({ fetch: team.fetch, token: () => "ibt" }))).toMatchObject({
      via: "token",
      team: { id: "t1", name: "Alpha" },
    });
    const admin = fakeFetch({ "GET /v1/team": [200, { admin: true }] });
    expect(await loadSession(new Api({ fetch: admin.fetch, token: () => "adm" }))).toMatchObject({
      via: "token",
      isAdmin: true,
    });
  });
});
