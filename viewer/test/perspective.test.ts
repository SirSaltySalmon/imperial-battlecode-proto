import { describe, expect, it } from "vitest";
import { parseMap } from "../src/map";
import { formatPerspective, parsePerspective, seenMask, unitVisibility, viewOf, viewers } from "../src/perspective";
import { CHARGE_DROPPED, CHARGE_PLANTED, DROP_CHARGE, DROP_WEAPON, S_BLINDED, S_HAS_CHARGE } from "../src/types";
import { angleDiff, canSeePoint, rulesFor, visionPolygon } from "../src/vision";
import { bullet, frame, MAP_TEXT, unit } from "./helpers";

const map = parseMap(MAP_TEXT);
const rules = rulesFor(null);

// Team A = ids 0, 1; team B = ids 2, 3. Unit 0 sees enemy 2; unit 1 sees enemy 3; unit 2 sees unit 0.
const units = () => [
  unit(0, 0, { seen: String(0b0110), x: 1536, y: 1536 }),
  unit(1, 0, { seen: String(0b1001), x: 2560, y: 4608 }),
  unit(2, 1, { seen: String(0b1001), x: 6656, y: 1536 }),
  unit(3, 1, { seen: "0", x: 8704, y: 4608 }),
];

describe("perspective", () => {
  it("round-trips the URL form", () => {
    for (const text of ["all", "a", "b", "u0", "u7"]) expect(formatPerspective(parsePerspective(text))).toBe(text);
    expect(parsePerspective(null)).toEqual({ kind: "all" });
    expect(parsePerspective("nonsense")).toEqual({ kind: "all" });
  });

  it("shows everything to everyone", () => {
    const f = frame({ units: units() });
    expect(unitVisibility(f, { kind: "all" })).toEqual([true, true, true, true]);
    expect(viewOf(f, { kind: "all" }, map, rules).viewerIds).toEqual([]);
  });

  it("a team sees enemies in the union of its units' seen masks", () => {
    const f = frame({ units: units() });
    expect(unitVisibility(f, { kind: "team", team: 0 })).toEqual([true, true, true, true]);
    f.units[1].seen = "1";
    expect(unitVisibility(f, { kind: "team", team: 0 })).toEqual([true, true, true, false]);
    expect(unitVisibility(f, { kind: "team", team: 1 })).toEqual([true, false, true, true]);
  });

  it("a single unit sees its team and only what it sees itself", () => {
    const f = frame({ units: units() });
    expect(unitVisibility(f, { kind: "unit", id: 0 })).toEqual([true, true, true, false]);
    expect(unitVisibility(f, { kind: "unit", id: 1 })).toEqual([true, true, false, true]);
    expect(unitVisibility(f, { kind: "unit", id: 3 })).toEqual([false, false, true, true]);
    expect(viewers(f, { kind: "unit", id: 1 }).map((u) => u.id)).toEqual([1]);
  });

  it("keeps dead enemies as markers and treats an unknown unit as everyone", () => {
    const f = frame({ units: units() });
    f.units[3].alive = false;
    expect(unitVisibility(f, { kind: "unit", id: 0 })[3]).toBe(true);
    expect(unitVisibility(f, { kind: "unit", id: 99 })).toEqual([true, true, true, true]);
  });

  it("handles masks above 2^53", () => {
    const high = unit(0, 0, { seen: (1n << 63n).toString() });
    expect(seenMask([high, unit(1, 0, { seen: "5" })])).toBe((1n << 63n) | 5n);
  });

  it("fogs bullets, effects and drops by what the viewers can see", () => {
    const f = frame({
      units: units(),
      // Unit 0 at (1.5, 1.5) faces +x along the open top row; the wall column at x = 4 hides row 4.
      projectiles: [
        bullet({ team: 1, owner: 2, x: 5632, y: 1536 }), // in view
        bullet({ team: 1, owner: 3, x: 7680, y: 4608 }), // behind the wall
        bullet({ team: 0, owner: 1, x: 7680, y: 4608 }), // own bullet: always known
      ],
      effects: [
        { kind: 1, team: 1, x: 5632, y: 1536, radius: 1024, ticks_left: 9, x2: 0, y2: 0 },
        { kind: 1, team: 1, x: 7680, y: 4608, radius: 1024, ticks_left: 9, x2: 0, y2: 0 },
      ],
      drops: [
        { kind: DROP_WEAPON, x: 7680, y: 4608, item: 1 },
        { kind: DROP_CHARGE, x: 7680, y: 4608, item: 0 },
      ],
    });
    const v = viewOf(f, { kind: "unit", id: 0 }, map, rules);
    expect(v.projectiles.map((p) => p.owner)).toEqual([2, 1]);
    expect(v.effects).toHaveLength(1);
    // Attackers always know where the charge lies; a weapon behind a wall stays hidden.
    expect(v.drops.map((d) => d.kind)).toEqual([DROP_CHARGE]);
    const defenders = viewOf({ ...f, attackers: 1 }, { kind: "unit", id: 0 }, map, rules);
    expect(defenders.drops).toEqual([]);
  });

  it("knows where the charge is only when the rules say so", () => {
    const f = frame({ units: units(), attackers: 1 });
    f.units[3].state |= S_HAS_CHARGE;
    const me = { kind: "unit", id: 0 } as const;
    expect(viewOf(f, me, map, rules).chargeVisible).toBe(false); // carrier (unit 3) is not seen by unit 0
    expect(viewOf(f, { kind: "unit", id: 1 }, map, rules).chargeVisible).toBe(true);
    const dropped = { ...f, charge: { state: CHARGE_DROPPED, x: 7680, y: 4608 } };
    expect(viewOf(dropped, me, map, rules).chargeVisible).toBe(false);
    expect(viewOf(dropped, { kind: "team", team: 1 }, map, rules).chargeVisible).toBe(true);
    const planted = { ...f, charge: { state: CHARGE_PLANTED, x: 7680, y: 4608 } };
    expect(viewOf(planted, me, map, rules).chargeVisible).toBe(true);
  });

  it("shows only the viewers' sounds and debug drawings", () => {
    const f = frame({ units: units(), debug: [[0, 2, 1, 1, 0, 0, 0xff0000], [2, 2, 1, 1, 0, 0, 0x00ff00]] });
    f.units[0].sounds = [[2, 100, 100]];
    f.units[1].sounds = [[2, 100, 100], [1, 5, 5]];
    f.units[2].sounds = [[1, 9, 9]];
    const a = viewOf(f, { kind: "team", team: 0 }, map, rules);
    expect(a.sounds).toEqual([[2, 100, 100], [1, 5, 5]]);
    expect(a.debug.map((d) => d[0])).toEqual([0]);
    expect(viewOf(f, { kind: "all" }, map, rules).sounds).toHaveLength(3);
  });
});

describe("vision", () => {
  const eye = unit(0, 0, { x: 1536, y: 1536, facing: 0 });

  it("wraps angle differences", () => {
    expect(angleDiff(0, 100)).toBe(100);
    expect(angleDiff(100, 0)).toBe(-100);
    expect(angleDiff(65000, 200)).toBe(736);
    expect(angleDiff(200, 65000)).toBe(-736);
    expect(angleDiff(0, 32768)).toBe(32768);
  });

  it("needs the cone, the range and a clear line; the close sense works all round", () => {
    expect(canSeePoint(map, rules, eye, 6656, 1536)).toBe(true);
    expect(canSeePoint(map, rules, { ...eye, facing: 32768 }, 6656, 1536)).toBe(false);
    expect(canSeePoint(map, rules, { ...eye, facing: 32768 }, 2560, 1536)).toBe(true); // within sense_radius
    expect(canSeePoint(map, rules, { ...eye, alive: false }, 2560, 1536)).toBe(false);
    expect(canSeePoint(map, rules, { ...eye, state: 1 | S_BLINDED }, 6656, 1536)).toBe(false);
    expect(canSeePoint(map, rules, { ...eye, state: 1 | S_BLINDED }, 2560, 1536)).toBe(true);
    expect(canSeePoint(map, { ...rules, vision_range: 2048 }, eye, 6656, 1536)).toBe(false);
    // Facing down-right at the wall column: the point behind it is hidden.
    expect(canSeePoint(map, rules, { ...eye, y: 2560, facing: 0 }, 6656, 2560)).toBe(false);
  });

  it("clips the vision outline to walls", () => {
    const poly = visionPolygon(map, rules, 1536, 1536, 0, false);
    for (const [x, y] of poly) {
      expect(x).toBeGreaterThanOrEqual(1024 - 1e-6);
      expect(x).toBeLessThanOrEqual(9 * 1024 + 1e-6);
      expect(y).toBeGreaterThanOrEqual(1024 - 1e-6);
      expect(y).toBeLessThanOrEqual(5 * 1024 + 1e-6);
    }
    // Straight ahead the top row is open up to the right wall.
    expect(Math.max(...poly.map((p) => p[0]))).toBeCloseTo(9 * 1024, 3);
    // A blinded unit keeps only the sense disc.
    const blind = visionPolygon(map, rules, 1536, 1536, 0, true);
    expect(Math.max(...blind.map((p) => Math.hypot(p[0] - 1536, p[1] - 1536)))).toBeLessThanOrEqual(rules.sense_radius + 1e-6);
  });
});
