// In-between positions for smooth playback. Rendering only: the simulation stays tick-exact, and a paused
// viewer (t = 0) always shows exactly what the frame says.

import { FULL_TURN, TILE, type ProjectileFrame, type UnitFrame, type Vec2 } from "./types";
import { angleDiff } from "./vision";

export function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}

/** Interpolate angles the short way round; the result is in 0..FULL_TURN. */
export function lerpAngle(a: number, b: number, t: number): number {
  const r = (a + angleDiff(a, b) * t) % FULL_TURN;
  return r < 0 ? r + FULL_TURN : r;
}

export interface Pose {
  x: number;
  y: number;
  facing: number;
}

/**
 * A unit that moves further than this in one tick did not walk there (round reset to spawn), so it snaps
 * instead of sliding across the map.
 */
export const MAX_GLIDE = 3 * TILE;

/** Poses at time `t` (0..1) between two frames, in the order of `cur`. `next` may be absent (last frame). */
export function interpUnits(cur: UnitFrame[], next: UnitFrame[] | null, t: number): Pose[] {
  return cur.map((a, i) => {
    const b = next?.[i];
    if (!b || t <= 0 || b.id !== a.id || !a.alive) return { x: a.x, y: a.y, facing: a.facing };
    if (Math.hypot(b.x - a.x, b.y - a.y) > MAX_GLIDE) return { x: a.x, y: a.y, facing: a.facing };
    return { x: lerp(a.x, b.x, t), y: lerp(a.y, b.y, t), facing: lerpAngle(a.facing, b.facing, t) };
  });
}

export interface Shot {
  team: number;
  /** `ProjectileInfo.kind`: 0 = a bullet, otherwise a thrown ability device (ability id + 1). */
  kind: number;
  /** Head of the bullet. */
  x: number;
  y: number;
  /** Tail of its trail. */
  tx: number;
  ty: number;
}

/** A bullet's start-of-tick position is recomputed from its velocity; allow for sub-step rounding. */
const MATCH_TOLERANCE = 96;
/** Longest trail drawn behind a bullet. */
export const TRAIL = 2.5 * TILE;

/** Where a bullet that is gone in the next frame stopped: the first wall, or a unit it hit. */
export type StopFn = (p: ProjectileFrame) => Vec2 | null;

/**
 * Bullets at time `t` between two frames.
 *  - A bullet of the next frame was at `pos - vel` one tick earlier, so it is drawn at `pos - vel * (1 - t)`.
 *    That covers both bullets that carry over and bullets fired during the tick (they leave the muzzle).
 *  - A bullet of the current frame with no successor hit something during the tick: it flies on until
 *    `stop` says where it ended, then disappears.
 */
export function interpShots(
  cur: ProjectileFrame[],
  next: ProjectileFrame[] | null,
  t: number,
  stop: StopFn = () => null,
): Shot[] {
  const out: Shot[] = [];
  const trail = (p: ProjectileFrame, x: number, y: number, travelled: number): Shot => {
    const speed = Math.hypot(p.vx, p.vy) || 1;
    const len = Math.min(TRAIL, travelled);
    return { team: p.team, kind: p.kind, x, y, tx: x - (p.vx / speed) * len, ty: y - (p.vy / speed) * len };
  };
  if (!next || t <= 0) {
    for (const p of cur) out.push(trail(p, p.x, p.y, TRAIL));
    return out;
  }
  const carried = new Array<boolean>(cur.length).fill(false);
  for (const q of next) {
    const sx = q.x - q.vx;
    const sy = q.y - q.vy;
    const from = cur.findIndex(
      (p, i) =>
        !carried[i] &&
        p.owner === q.owner &&
        p.vx === q.vx &&
        p.vy === q.vy &&
        Math.abs(p.x - sx) <= MATCH_TOLERANCE &&
        Math.abs(p.y - sy) <= MATCH_TOLERANCE,
    );
    if (from >= 0) carried[from] = true;
    const speed = Math.hypot(q.vx, q.vy);
    // A fresh bullet has only flown `speed * t` since it left the muzzle.
    out.push(trail(q, q.x - q.vx * (1 - t), q.y - q.vy * (1 - t), from >= 0 ? TRAIL : speed * t));
  }
  cur.forEach((p, i) => {
    if (carried[i]) return;
    const end = stop(p);
    const x = p.x + p.vx * t;
    const y = p.y + p.vy * t;
    if (end) {
      const reach = Math.hypot(end[0] - p.x, end[1] - p.y);
      if (Math.hypot(p.vx, p.vy) * t >= reach) return;
    }
    out.push(trail(p, x, y, TRAIL));
  });
  return out;
}

/** Fraction of the way from `a` to `b` at which the segment passes closest to `p`, clamped to 0..1. */
export function closestOnSegment(a: Vec2, b: Vec2, p: Vec2): number {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  const len2 = dx * dx + dy * dy;
  if (len2 === 0) return 0;
  return Math.max(0, Math.min(1, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2));
}
