import { describe, expect, it } from "vitest";
import { aimReach } from "../src/draw-units";
import { parseMap } from "../src/map";
import { TILE } from "../src/types";
import { rulesFor } from "../src/vision";

// 12 tiles wide, a wall column at x = 8.
const map = parseMap("name: reach\n---\n############\n#.......#..#\n#..........#\n############\n");
const rules = rulesFor({ weapon_ranges: [3 * TILE, 20 * TILE] });

describe("wind-up aim line", () => {
  it("reaches as far as the weapon does in the open", () => {
    expect(aimReach(map, rules, 0, [1.5 * TILE, 2.5 * TILE], 0)).toEqual({ len: 3 * TILE, wall: false });
  });

  it("stops at the first wall in the way", () => {
    const reach = aimReach(map, rules, 1, [1.5 * TILE, 1.5 * TILE], 0);
    expect(reach.wall).toBe(true);
    expect(reach.len).toBeCloseTo(6.5 * TILE);
  });

  it("falls back to a fixed length when the rules do not say", () => {
    expect(aimReach(map, rulesFor(null), 0, [1.5 * TILE, 2.5 * TILE], 0)).toEqual({ len: 5 * TILE, wall: false });
  });
});
