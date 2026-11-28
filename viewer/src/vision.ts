// What a unit can see, for drawing cones and for fogging things that carry no `seen` bit (bullets, effects,
// drops). Mirrors `World::can_see_point` in the engine, in floats; a team sees the union over its living units
// (`World::team_sees_point`).
//
// The numbers and names the viewer needs come from the engine build that plays the replay (`Engine.rules`).
// The table below is only the fallback for a build that predates that export.

import { segmentWall, type GameMap } from "./map";
import { FULL_TURN, S_BLINDED, type UnitFrame, type Vec2 } from "./types";

export interface ViewRules {
  unit_radius: number;
  fov_half: number;
  vision_range: number;
  sense_radius: number;
  max_hp: number;
  plant_ticks: number;
  defuse_ticks: number;
  fuse_ticks: number;
  /** Detonation blast radius; absent for a build that predates it (the viewer then draws no radius). */
  blast_radius?: number;
  /** Weapon names by weapon id. */
  weapons: string[];
  /** How far each weapon's bullets fly, by weapon id. Empty for a build that predates it. */
  weapon_ranges: number[];
}

const RULES_DEFAULT: ViewRules = {
  unit_radius: 300,
  fov_half: 9100,
  vision_range: 24576,
  sense_radius: 1536,
  max_hp: 100,
  plant_ticks: 12,
  defuse_ticks: 21,
  fuse_ticks: 80,
  weapons: ["pistol", "rifle", "smg", "shotgun", "marksman", "lmg", "hunter"],
  weapon_ranges: [],
};

/** The rules to draw with: what the engine reports, over the fallback for anything it does not. */
export function rulesFor(fromEngine: Partial<ViewRules> | null): ViewRules {
  return { ...RULES_DEFAULT, ...fromEngine };
}

export function weaponName(rules: ViewRules, id: number): string {
  if (id < 0) return "unarmed";
  return rules.weapons[id] ?? `weapon ${id}`;
}

/** Signed shortest difference b - a in angle units, in (-32768, 32768]. */
export function angleDiff(a: number, b: number): number {
  let d = (b - a) % FULL_TURN;
  if (d > FULL_TURN / 2) d -= FULL_TURN;
  if (d <= -FULL_TURN / 2) d += FULL_TURN;
  return d;
}

export function toRadians(angle: number): number {
  return (angle / FULL_TURN) * Math.PI * 2;
}

type Viewer = Pick<UnitFrame, "alive" | "x" | "y" | "facing" | "state">;

/** Cone + range + line of sight, or the close all-round sense. */
export function canSeePoint(map: GameMap, rules: ViewRules, u: Viewer, px: number, py: number): boolean {
  if (!u.alive) return false;
  const dx = px - u.x;
  const dy = py - u.y;
  const d2 = dx * dx + dy * dy;
  if (d2 > rules.sense_radius ** 2) {
    if (u.state & S_BLINDED) return false;
    if (d2 > rules.vision_range ** 2) return false;
    const ang = (Math.atan2(dy, dx) / (Math.PI * 2)) * FULL_TURN;
    if (Math.abs(angleDiff(u.facing, ang)) > rules.fov_half) return false;
  }
  return segmentWall(map, u.x, u.y, px, py) === null;
}

/**
 * Outline of the area a unit sees: the cone out to `vision_range` joined with the `sense_radius` disc, both
 * cut by walls. Rays are cast all the way round; `facing` may be fractional (interpolated).
 */
export function visionPolygon(
  map: GameMap,
  rules: ViewRules,
  x: number,
  y: number,
  facing: number,
  blinded: boolean,
): Vec2[] {
  const out: Vec2[] = [];
  const cast = (angle: number, range: number) => {
    const r = toRadians(angle);
    const bx = x + Math.cos(r) * range;
    const by = y + Math.sin(r) * range;
    out.push(segmentWall(map, x, y, bx, by) ?? [bx, by]);
  };
  const half = blinded ? 0 : rules.fov_half;
  const CONE_RAYS = 40;
  const RING_RAYS = 28;
  if (half > 0) {
    for (let i = 0; i <= CONE_RAYS; i++) cast(facing - half + (2 * half * i) / CONE_RAYS, rules.vision_range);
  }
  const rest = FULL_TURN - 2 * half;
  for (let i = 0; i <= RING_RAYS; i++) cast(facing + half + (rest * i) / RING_RAYS, rules.sense_radius);
  return out;
}
