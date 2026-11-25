import { describe, expect, it } from "vitest";
import { fitCamera, toTiles, zoomAt } from "../src/camera";
import { closestOnSegment, interpShots, interpUnits, lerp, lerpAngle, MAX_GLIDE, TRAIL } from "../src/interp";
import { bullet, unit } from "./helpers";

describe("unit interpolation", () => {
  it("lerps numbers and takes the short way round for angles", () => {
    expect(lerp(10, 20, 0.25)).toBe(12.5);
    expect(lerpAngle(0, 16384, 0.5)).toBe(8192);
    expect(lerpAngle(65000, 536, 0.5)).toBe(0);
    expect(lerpAngle(536, 65000, 0.5)).toBe(0);
    expect(lerpAngle(100, 65436, 0.75)).toBe(65486);
  });

  it("moves and turns units between frames", () => {
    const cur = [unit(0, 0, { x: 1000, y: 1000, facing: 0 })];
    const next = [unit(0, 0, { x: 1400, y: 800, facing: 2000 })];
    expect(interpUnits(cur, next, 0.5)).toEqual([{ x: 1200, y: 900, facing: 1000 }]);
  });

  it("shows the frame exactly at t = 0 and on the last frame", () => {
    const cur = [unit(0, 0, { x: 1000, y: 1000, facing: 123 })];
    const next = [unit(0, 0, { x: 1400, y: 1000, facing: 456 })];
    expect(interpUnits(cur, next, 0)).toEqual([{ x: 1000, y: 1000, facing: 123 }]);
    expect(interpUnits(cur, null, 0.7)).toEqual([{ x: 1000, y: 1000, facing: 123 }]);
  });

  it("does not slide a unit across the map on a round reset, nor move the dead", () => {
    const cur = [unit(0, 0, { x: 1000, y: 1000 }), unit(1, 0, { x: 5000, y: 5000, alive: false })];
    const next = [unit(0, 0, { x: 1000 + MAX_GLIDE + 1, y: 1000 }), unit(1, 0, { x: 5200, y: 5000 })];
    const poses = interpUnits(cur, next, 0.5);
    expect(poses[0].x).toBe(1000);
    expect(poses[1].x).toBe(5000);
  });
});

describe("bullet interpolation", () => {
  it("draws the frame's bullets as they are when paused", () => {
    const shots = interpShots([bullet({ x: 5000, y: 2000, vx: 1000 })], null, 0);
    expect(shots).toEqual([{ team: 0, x: 5000, y: 2000, tx: 5000 - TRAIL, ty: 2000 }]);
  });

  it("carries a bullet over to its next position", () => {
    const cur = [bullet({ x: 2000, vx: 8000 })];
    const next = [bullet({ x: 10000, vx: 8000 })];
    const shots = interpShots(cur, next, 0.25);
    expect(shots).toHaveLength(1);
    expect(shots[0].x).toBe(4000);
    expect(shots[0].tx).toBe(4000 - TRAIL);
  });

  it("tolerates sub-step rounding when matching", () => {
    const shots = interpShots([bullet({ x: 2000, vx: 8001 })], [bullet({ x: 9994, vx: 8001 })], 0.5);
    expect(shots).toHaveLength(1);
  });

  it("lets a new bullet leave from where it was fired, with a trail no longer than its flight", () => {
    const shots = interpShots([], [bullet({ x: 10000, vx: 8000 })], 0.1);
    expect(shots).toHaveLength(1);
    expect(shots[0].x).toBeCloseTo(2800);
    expect(shots[0].x - shots[0].tx).toBeCloseTo(800);
  });

  it("flies a vanishing bullet up to where it stopped, then drops it", () => {
    const cur = [bullet({ x: 2000, y: 2000, vx: 8000 })];
    const stop = () => [6000, 2000] as [number, number];
    expect(interpShots(cur, [], 0.25, stop).map((s) => s.x)).toEqual([4000]);
    expect(interpShots(cur, [], 0.5, stop)).toEqual([]);
    expect(interpShots(cur, [], 0.9, () => null).map((s) => s.x)).toEqual([9200]);
  });

  it("matches each bullet once", () => {
    const cur = [bullet({ x: 2000 }), bullet({ x: 2000 })];
    const next = [bullet({ x: 3000 })];
    // One carries over, the other one vanished during the tick.
    expect(interpShots(cur, next, 0.5)).toHaveLength(2);
    expect(interpShots(cur, next, 0.5, () => [2100, 2000])).toHaveLength(1);
  });

  it("projects a point onto a segment", () => {
    expect(closestOnSegment([0, 0], [10, 0], [5, 3])).toBe(0.5);
    expect(closestOnSegment([0, 0], [10, 0], [-4, 3])).toBe(0);
    expect(closestOnSegment([0, 0], [10, 0], [40, 3])).toBe(1);
    expect(closestOnSegment([2, 2], [2, 2], [9, 9])).toBe(0);
  });
});

describe("camera", () => {
  it("fits the map and keeps the point under the cursor when zooming", () => {
    const cam = fitCamera(32, 17, 1000, 600);
    expect(cam.cx).toBe(16);
    expect(cam.ppt).toBeCloseTo(968 / 32);
    const before = toTiles(cam, 300, 200, 1000, 600);
    const zoomed = zoomAt(cam, 2, 300, 200, 1000, 600);
    const after = toTiles(zoomed, 300, 200, 1000, 600);
    expect(after[0]).toBeCloseTo(before[0]);
    expect(after[1]).toBeCloseTo(before[1]);
    expect(zoomed.ppt).toBeCloseTo(cam.ppt * 2);
  });
});
