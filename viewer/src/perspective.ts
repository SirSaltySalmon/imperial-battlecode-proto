// Whose eyes the map is drawn through. "all" shows everything. Vision and hearing are shared by the team (the
// engine fogs every bot's tables per team), so a team and a single unit see the same thing: their own team in
// full and only the enemies in the union of the team's `seen` bitmasks. A single unit differs only in showing
// just its own debug drawings.

import type { GameMap } from "./map";
import {
  CHARGE_CARRIED,
  CHARGE_DROPPED,
  DROP_CHARGE,
  EFFECT_TRAP,
  S_HAS_CHARGE,
  type DebugDraw,
  type DropFrame,
  type EffectFrame,
  type Frame,
  type ProjectileFrame,
  type UnitFrame,
} from "./types";
import { canSeePoint, type ViewRules } from "./vision";

export type Perspective = { kind: "all" } | { kind: "team"; team: number } | { kind: "unit"; id: number };

export const ALL: Perspective = { kind: "all" };

/** `all`, `a`, `b` or `u<id>`: the form used in the page URL. */
export function parsePerspective(text: string | null): Perspective {
  if (text === "a" || text === "b") return { kind: "team", team: text === "a" ? 0 : 1 };
  const m = /^u(\d+)$/.exec(text ?? "");
  return m ? { kind: "unit", id: Number(m[1]) } : ALL;
}

export function formatPerspective(p: Perspective): string {
  if (p.kind === "team") return p.team === 0 ? "a" : "b";
  return p.kind === "unit" ? `u${p.id}` : "all";
}

/** The team a fogged perspective belongs to, or null for "all" (and for a unit id that does not exist). */
export function perspectiveTeam(frame: Frame, p: Perspective): number | null {
  if (p.kind === "team") return p.team;
  if (p.kind === "unit") return frame.units.find((u) => u.id === p.id)?.team ?? null;
  return null;
}

/** Units whose vision the perspective uses: the whole team of a team or unit perspective. */
export function viewers(frame: Frame, p: Perspective): UnitFrame[] {
  const team = perspectiveTeam(frame, p);
  return team === null ? frame.units : frame.units.filter((u) => u.team === team);
}

/** Units whose debug drawings the perspective shows. */
function debugIds(frame: Frame, p: Perspective): Set<number> {
  if (p.kind === "unit") return new Set([p.id]);
  return new Set(viewers(frame, p).map((u) => u.id));
}

/** Union of the viewers' `seen` masks (bit i = unit id i). */
export function seenMask(units: UnitFrame[]): bigint {
  let mask = 0n;
  for (const u of units) mask |= BigInt(u.seen);
  return mask;
}

/**
 * Per unit (same order as `frame.units`): is it drawn? Own team: always. Enemies: only while alive and in
 * the viewers' `seen` mask. A dead enemy is kept here (kills are public events); `viewOf` then draws its
 * body only where the viewers can see it, so an unseen kill does not give away the spot.
 */
export function unitVisibility(frame: Frame, p: Perspective): boolean[] {
  const team = perspectiveTeam(frame, p);
  if (team === null) return frame.units.map(() => true);
  const mask = seenMask(viewers(frame, p));
  return frame.units.map((u) => u.team === team || !u.alive || ((mask >> BigInt(u.id)) & 1n) === 1n);
}

export interface View {
  /** Same order as `frame.units`. */
  unitVisible: boolean[];
  projectiles: ProjectileFrame[];
  effects: EffectFrame[];
  drops: DropFrame[];
  /** Deduplicated `[kind, x, y]` heard by the viewers. */
  sounds: [number, number, number][];
  debug: DebugDraw[];
  /** Whether the charge marker (carried, dropped or planted) is known to the viewers. */
  chargeVisible: boolean;
  /** Ids of the units whose vision is used; empty for "all". */
  viewerIds: number[];
  /** The viewers' team, or null for "all". */
  team: number | null;
  /** Can any viewer see this point right now? Always true for "all". */
  canSee(x: number, y: number): boolean;
}

/** Everything of one frame that the perspective is allowed to show. */
export function viewOf(frame: Frame, p: Perspective, map: GameMap, rules: ViewRules): View {
  const team = perspectiveTeam(frame, p);
  const eyes = viewers(frame, p);
  const unitVisible = unitVisibility(frame, p);
  const sounds = dedupeSounds(eyes);
  if (team === null) {
    return {
      unitVisible,
      projectiles: frame.projectiles,
      effects: frame.effects,
      drops: frame.drops,
      sounds,
      debug: frame.debug,
      chargeVisible: true,
      viewerIds: [],
      team: null,
      canSee: () => true,
    };
  }
  const sees = (x: number, y: number) => eyes.some((u) => canSeePoint(map, rules, u, x, y));
  // Kills are public, but not where they happened: a dead enemy is drawn only while its body is in sight.
  frame.units.forEach((u, i) => {
    if (!u.alive && u.team !== team) unitVisible[i] = sees(u.x, u.y);
  });
  // An enemy trap shows only to a viewer within `sense_radius` of it (docs/rules/abilities.md).
  const near = (x: number, y: number) =>
    eyes.some((u) => (u.x - x) ** 2 + (u.y - y) ** 2 <= rules.sense_radius ** 2 && canSeePoint(map, rules, u, x, y));
  const effectVisible = (e: EffectFrame) => e.team === team || (e.kind === EFFECT_TRAP ? near : sees)(e.x, e.y);
  const attacking = frame.attackers === team;
  const ids = new Set(eyes.map((u) => u.id));
  const drawers = debugIds(frame, p);
  let chargeVisible = true;
  if (frame.charge.state === CHARGE_CARRIED) {
    const carrier = frame.units.findIndex((u) => u.state & S_HAS_CHARGE);
    chargeVisible = carrier >= 0 && unitVisible[carrier];
  } else if (frame.charge.state === CHARGE_DROPPED) {
    chargeVisible = attacking || sees(frame.charge.x, frame.charge.y);
  }
  return {
    unitVisible,
    projectiles: frame.projectiles.filter((q) => q.team === team || sees(q.x, q.y)),
    effects: frame.effects.filter(effectVisible),
    drops: frame.drops.filter((d) => (d.kind === DROP_CHARGE && attacking) || sees(d.x, d.y)),
    sounds,
    debug: frame.debug.filter((d) => drawers.has(d[0])),
    chargeVisible,
    viewerIds: [...ids],
    team,
    canSee: sees,
  };
}

function dedupeSounds(units: UnitFrame[]): [number, number, number][] {
  const seen = new Set<string>();
  const out: [number, number, number][] = [];
  for (const u of units) {
    for (const s of u.sounds) {
      const key = `${s[0]},${s[1]},${s[2]}`;
      if (!seen.has(key)) {
        seen.add(key);
        out.push(s);
      }
    }
  }
  return out;
}
