import { describe, expect, it } from "vitest";
import { Player, type Track } from "../src/player";

/** A player on a track of `count` frames that holds `holds[i]` ticks on frame i; nothing is drawn. */
function playerOn(count: number, holds: Record<number, number>): Player {
  const canvas = { getContext: () => ({}) } as unknown as HTMLCanvasElement;
  const player = new Player(canvas, {} as HTMLElement, { palette: () => ({}) as never, page: () => {}, playing: () => {} });
  player.track = { count, hold: (i: number) => holds[i] ?? 0 } as unknown as Track;
  return player;
}

describe("playback holds", () => {
  it("waits on a held frame before moving on, then carries the rest of the step over", () => {
    const p = playerOn(10, { 3: 5 });
    expect(p.advance(2.5)).toBe(true);
    expect(p.pos).toBe(2.5);
    expect(p.advance(0.5)).toBe(true);
    expect([p.pos, p.held]).toEqual([3, 0]);
    expect(p.advance(4)).toBe(true);
    expect([p.pos, p.held]).toEqual([3, 4]);
    expect(p.advance(1.5)).toBe(true);
    expect([p.pos, p.held]).toEqual([3.5, 5]);
  });

  it("stops at the final frame once its hold is done", () => {
    const p = playerOn(4, { 3: 5 });
    expect(p.advance(7)).toBe(true);
    expect([p.pos, p.held]).toEqual([3, 4]);
    expect(p.advance(2)).toBe(false);
    expect([p.pos, p.held]).toEqual([3, 5]);
  });

  it("plays straight through a track without holds", () => {
    const p = playerOn(4, {});
    expect(p.advance(2)).toBe(true);
    expect(p.pos).toBe(2);
    expect(p.advance(5)).toBe(false);
    expect(p.pos).toBe(3);
  });
});
