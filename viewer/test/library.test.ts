import { describe, expect, it } from "vitest";
import { currentIndex, entryHref, entryLabel, entryUrl, neighbour, type LibraryEntry } from "../src/library";

const entry = (path: string, over: Partial<LibraryEntry> = {}): LibraryEntry => ({
  path,
  bots: ["sapper", "retaker"],
  map: "crossfire",
  score: [7, 4],
  winner: 0,
  ticks: 900,
  finished: true,
  ...over,
});

describe("replay library", () => {
  const list = [entry("a.ibcr"), entry("nested/good one.ibcr"), entry("z.ibcr", { finished: false })];

  it("labels an entry with who played, the score and the map", () => {
    expect(entryLabel(list[0])).toBe("sapper 7-4 retaker · crossfire");
    expect(entryLabel(list[2])).toBe("sapper unfinished retaker · crossfire");
  });

  it("builds a relative, escaped url that the page parameter round-trips", () => {
    expect(entryUrl(list[1])).toBe("replays/nested/good%20one.ibcr");
    expect(entryHref(list[1])).toBe("?replay=replays%2Fnested%2Fgood%2520one.ibcr");
    const param = new URLSearchParams(entryHref(list[1])).get("replay");
    expect(currentIndex(list, param)).toBe(1);
  });

  it("finds the open replay however the parameter was written", () => {
    expect(currentIndex(list, "replays/a.ibcr")).toBe(0);
    expect(currentIndex(list, "/replays/a.ibcr")).toBe(0);
    expect(currentIndex(list, "./replays/nested/good one.ibcr")).toBe(1);
    expect(currentIndex(list, "/replay.ibcr")).toBe(-1);
    expect(currentIndex(list, null)).toBe(-1);
  });

  it("steps to the neighbours and wraps", () => {
    expect(neighbour(list, 0, 1)).toBe(list[1]);
    expect(neighbour(list, 0, -1)).toBe(list[2]);
    expect(neighbour(list, 2, 1)).toBe(list[0]);
    expect(neighbour(list, -1, 1)).toBe(list[0]);
    expect(neighbour(list, -1, -1)).toBe(list[2]);
    expect(neighbour([], -1, 1)).toBeNull();
  });
});
