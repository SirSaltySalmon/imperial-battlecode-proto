import { describe, expect, it } from "vitest";
import type { PlayUnit } from "../src/play/api";
import { blend, interpFrom, tickPeriod, TURN_GLIDE_MS } from "../src/play/smooth";
import type { Frame } from "../src/types";

const unit = (id: number, x: number, extra: Partial<PlayUnit> = {}): PlayUnit =>
  ({ id, x, y: 0, alive: true, facing: 0, ...extra }) as PlayUnit;
const frame = (units: PlayUnit[]): Frame => ({ units }) as unknown as Frame;

describe("play smoothing", () => {
  it("draws a tick over the clock's period, or a short glide in turn mode", () => {
    expect(tickPeriod(10)).toBe(100);
    expect(tickPeriod(0)).toBe(TURN_GLIDE_MS);
    expect(blend(0, 100)).toBe(0);
    expect(blend(50, 100)).toBe(0.5);
    expect(blend(250, 100)).toBe(1);
    expect(blend(-5, 100)).toBe(0);
    expect(blend(10, 0)).toBe(1);
  });

  it("never glides a unit out of the fog", () => {
    const prev = frame([unit(0, 100), unit(1, 0, { hidden: true }), unit(2, 300)]);
    const cur = frame([unit(0, 150), unit(1, 5000), unit(2, 0, { hidden: true })]);
    const from = interpFrom(prev, cur).units as PlayUnit[];
    expect(from[0].x).toBe(100);
    expect(from[1].x).toBe(5000);
    expect(from[2].hidden).toBe(true);
  });
});
