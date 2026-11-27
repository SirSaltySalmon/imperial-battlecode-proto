// Smooth drawing between the server's ticks (entity interpolation): the page draws the previous tick moving
// towards the latest one over one tick period, so units and bullets glide instead of jumping once per tick.
// What is drawn trails the server by at most one tick; the HUD and the controls always use the latest.

import type { Frame } from "../types";
import type { PlayUnit } from "./api";

/** How long a tick's motion is drawn over in turn mode, where ticks have no period of their own. */
export const TURN_GLIDE_MS = 120;

/** Milliseconds one tick is drawn over: the clock's period, or a short glide in turn mode (`tps` 0). */
export function tickPeriod(tps: number): number {
  return tps > 0 ? 1000 / tps : TURN_GLIDE_MS;
}

/** How far (0..1) the drawing is from the previous tick to the latest, `sinceMs` after the latest arrived. */
export function blend(sinceMs: number, periodMs: number): number {
  if (!(periodMs > 0)) return 1;
  return Math.min(1, Math.max(0, sinceMs / periodMs));
}

/**
 * The previous frame, safe to interpolate from towards `cur`. A unit hidden by the fog in either frame takes
 * its row from `cur`: a blanked row sits at the origin, and gliding from there would sweep a unit across
 * the map (or reveal where a hidden one went).
 */
export function interpFrom(prev: Frame, cur: Frame): Frame {
  const units = (prev.units as PlayUnit[]).map((u, i) => {
    const c = cur.units[i] as PlayUnit | undefined;
    return c && (u.hidden || c.hidden || u.id !== c.id) ? c : u;
  });
  return { ...prev, units };
}
