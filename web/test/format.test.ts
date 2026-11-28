import { describe, expect, it } from "vitest";
import { ApiError, type Match, type Pairing } from "../src/api";
import {
  cellText,
  checkTournamentMaps,
  decidedText,
  errorText,
  formatBytes,
  formatDuration,
  isFlagged,
  objectColumns,
  pairingSide,
  resultFor,
  safeUrl,
  scoreText,
  splitList,
  tierLabel,
  viewerHref,
  replayPath,
} from "../src/format";

const match = (over: Partial<Match> = {}): Match => ({
  match_id: "m1",
  series_id: "s1",
  index: 0,
  kind: "ranked",
  map: "crossfire",
  seed: 1,
  bot_a: { team_id: "a", name: "Alpha" },
  bot_b: { team_id: "b", name: "Beta" },
  state: "done",
  winner: 0,
  winner_team: "a",
  outcome: "a",
  score: [7, 4],
  ticks: 900,
  replay_url: "/v1/matches/m1/replay",
  verified: false,
  created_at: "2026-11-25T10:00:00Z",
  finished_at: "2026-11-25T10:01:00Z",
  ...over,
});

const pairing = (over: Partial<Pairing> = {}): Pairing => ({
  a: "t1",
  b: "t2",
  seed_a: 1,
  seed_b: 8,
  maps_won: [1, 1],
  rounds_won: [20, 20],
  winner: "t1",
  decided_by: "seed",
  ...over,
});

describe("errors as text", () => {
  it("says not available yet for missing endpoints", () => {
    expect(errorText(new ApiError(404, "", "HTTP 404", null))).toMatch(/not available yet/i);
  });
  it("gives the retry time of a 429", () => {
    expect(errorText(new ApiError(429, "rate_limited", "Too many uploads.", 125))).toBe(
      "Too many uploads. Try again in 3 min.",
    );
    expect(errorText(new ApiError(429, "rate_limited", "slow down", null))).toBe("slow down. Try again later.");
  });
  it("passes other messages through", () => {
    expect(errorText(new ApiError(400, "bad_request", "games must be between 1 and 5", null))).toBe(
      "games must be between 1 and 5",
    );
    expect(errorText(new Error("boom"))).toBe("boom");
  });
});

describe("labels", () => {
  it("shows Provisional before a tier", () => {
    expect(tierLabel({ tier: "Gold", provisional: true })).toBe("Provisional");
    expect(tierLabel({ tier: "Radiant", provisional: false })).toBe("Radiant");
    expect(tierLabel({ tier: null, provisional: false })).toBe("—");
  });
  it("formats durations and sizes", () => {
    expect(formatDuration(45)).toBe("45 s");
    expect(formatDuration(60)).toBe("1 min");
    expect(formatDuration(3600 + 300)).toBe("1 h 5 min");
    expect(formatBytes(2048)).toBe("2.0 KiB");
  });
});

describe("matches", () => {
  it("links the embedded viewer to the same-origin replay", () => {
    expect(viewerHref("m 1")).toBe("/viewer/?replay=/v1/matches/m%201/replay");
  });
  it("asks for the team view of a match the signed-in team played", () => {
    expect(replayPath("m1", "team")).toBe("/v1/matches/m1/replay?view=team");
    const href = viewerHref("m1", "team");
    expect(new URL(href, "http://x").searchParams.get("replay")).toBe("/v1/matches/m1/replay?view=team");
  });
  it("scores and results", () => {
    expect(scoreText(match())).toBe("7–4");
    expect(scoreText(match({ state: "queued", score: null }))).toBe("queued");
    expect(resultFor(match(), "a")).toBe("won");
    expect(resultFor(match(), "b")).toBe("lost");
    expect(resultFor(match({ outcome: "draw" }), "b")).toBe("draw");
    expect(resultFor(match(), "c")).toBe("");
  });
});

describe("brackets", () => {
  it("flags seed decisions and says how a pairing was decided", () => {
    expect(isFlagged(pairing())).toBe(true);
    expect(isFlagged(pairing({ decided_by: "maps" }))).toBe(false);
    expect(decidedText(pairing({ decided_by: "maps", maps_won: [2, 1] }))).toBe("maps 2–1");
    expect(decidedText(pairing({ decided_by: "rounds" }))).toBe("rounds 20–20");
    expect(decidedText(pairing({ decided_by: null }))).toBe("");
  });
  it("names sides", () => {
    expect(pairingSide(pairing({ a_name: "Alpha" }), "a")).toBe("Alpha");
    expect(pairingSide(pairing(), "b")).toBe("t2");
    expect(pairingSide(pairing({ b: null }), "b")).toBe("bye");
    expect(pairingSide(pairing({ b: null, state: "bye" }), "b")).toBe("bye");
    expect(pairingSide(pairing({ b: null, state: "waiting" }), "b")).toBe("to be decided");
  });
  it("wants an odd map count", () => {
    expect(checkTournamentMaps([])).not.toBeNull();
    expect(checkTournamentMaps(["a", "b"])).toMatch(/odd/);
    expect(checkTournamentMaps(["a", "b", "c"])).toBeNull();
    expect(splitList("a, b ,, c")).toEqual(["a", "b", "c"]);
  });
});

describe("generic tables", () => {
  it("collects columns and renders cells", () => {
    expect(objectColumns([{ a: 1 }, { b: 2, a: 3 }])).toEqual(["a", "b"]);
    expect(cellText(null)).toBe("—");
    expect(cellText({ x: 1 })).toBe('{"x":1}');
  });
  it("only links safe URLs", () => {
    expect(safeUrl("/dl/ibc.tar.gz")).toBe("/dl/ibc.tar.gz");
    expect(safeUrl("https://example.org/x")).toBe("https://example.org/x");
    expect(safeUrl("javascript:alert(1)")).toBeNull();
    expect(safeUrl("//evil.example/x")).toBeNull();
  });
});
