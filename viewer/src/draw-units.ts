// Units: body, facing, bars and state marks.
//
// Team A is a solid disc, team B a ring, so the teams differ in shape as well as colour.

import type { Pose } from "./interp";
import { FONT, type Pen } from "./render";
import { alpha } from "./theme";
import {
  S_BLINDED,
  S_CASTING,
  S_DEFUSING,
  S_PLANTING,
  S_RELOADING,
  S_REVEALED,
  S_SLOWED,
  S_WINDUP,
  TILE,
  type UnitFrame,
  type Vec2,
} from "./types";
import { toRadians, weaponName, type ViewRules } from "./vision";
import { segmentWall, type GameMap } from "./map";

const K = 1 / TILE;
/** Length of the wind-up aim line, in tiles, when the rules do not say how far the weapon reaches. */
const AIM_LINE = 5;

/**
 * How far a shot aimed at `aim` (radians) from `from` can go: the weapon's range, or less if a wall is in the
 * way (`wall`). Lengths in fixed units, like positions.
 */
export function aimReach(map: GameMap, rules: ViewRules, weapon: number, from: Vec2, aim: number): { len: number; wall: boolean } {
  const range = rules.weapon_ranges[weapon] ?? AIM_LINE * TILE;
  const to: Vec2 = [from[0] + Math.cos(aim) * range, from[1] + Math.sin(aim) * range];
  const hit = segmentWall(map, from[0], from[1], to[0], to[1]);
  return hit ? { len: Math.hypot(hit[0] - from[0], hit[1] - from[1]), wall: true } : { len: range, wall: false };
}

/** "A1".."A5", "B1".."B5": team letter and slot within the team. */
export function unitLabel(u: Pick<UnitFrame, "id" | "team">, teamSize: number): string {
  return `${u.team === 0 ? "A" : "B"}${u.id - u.team * teamSize + 1}`;
}

export function drawUnits(pen: Pen, poses: Pose[]): void {
  const { frame, view } = pen.scene;
  // Dead first, so the living are never covered by a marker.
  frame.units.forEach((u, i) => {
    if (!u.alive && view.unitVisible[i] && frame.phase !== 0) drawDead(pen, u, poses[i]);
  });
  frame.units.forEach((u, i) => {
    if (u.alive && view.unitVisible[i]) drawAlive(pen, u, poses[i]);
  });
}

function drawDead(pen: Pen, u: UnitFrame, pose: Pose): void {
  const { ctx, px } = pen;
  const r = pen.scene.rules.unit_radius * K * 0.7;
  const x = pose.x * K;
  const y = pose.y * K;
  ctx.strokeStyle = alpha(pen.scene.palette.team[u.team], 0.5);
  ctx.lineWidth = 2.5 * px;
  ctx.beginPath();
  ctx.moveTo(x - r, y - r);
  ctx.lineTo(x + r, y + r);
  ctx.moveTo(x + r, y - r);
  ctx.lineTo(x - r, y + r);
  ctx.stroke();
}

function drawAlive(pen: Pen, u: UnitFrame, pose: Pose): void {
  const { ctx, px } = pen;
  const { palette: pal, rules, camera, frame } = pen.scene;
  const x = pose.x * K;
  const y = pose.y * K;
  const r = rules.unit_radius * K;
  const color = pal.team[u.team];
  const face = toRadians(pose.facing);

  if (u.state & S_WINDUP) {
    const aim = toRadians(u.aim);
    const reach = aimReach(pen.scene.map, rules, u.weapon, [pose.x, pose.y], aim);
    const [cx, cy] = [Math.cos(aim), Math.sin(aim)];
    const [ex, ey] = [x + cx * reach.len * K, y + cy * reach.len * K];
    ctx.strokeStyle = alpha(pal.warn, 0.85);
    ctx.lineWidth = 1.5 * px;
    ctx.setLineDash([5 * px, 4 * px]);
    ctx.beginPath();
    ctx.moveTo(x + cx * r, y + cy * r);
    ctx.lineTo(ex, ey);
    ctx.stroke();
    ctx.setLineDash([]);
    if (!reach.wall) {
      // The end of the weapon's reach, square to the line.
      ctx.beginPath();
      ctx.moveTo(ex - cy * r, ey + cx * r);
      ctx.lineTo(ex + cy * r, ey - cx * r);
      ctx.stroke();
    }
  }
  if (pen.scene.selected === u.id) {
    ctx.beginPath();
    ctx.arc(x, y, r * 2, 0, Math.PI * 2);
    ctx.strokeStyle = alpha(pal.text, 0.9);
    ctx.lineWidth = 1.5 * px;
    ctx.stroke();
  }

  // Barrel, then the body over its root.
  ctx.strokeStyle = color;
  ctx.lineWidth = Math.max(r * 0.42, 2.5 * px);
  ctx.lineCap = "butt";
  ctx.beginPath();
  ctx.moveTo(x, y);
  ctx.lineTo(x + Math.cos(face) * r * 1.75, y + Math.sin(face) * r * 1.75);
  ctx.stroke();
  ctx.lineCap = "round";
  ctx.beginPath();
  ctx.arc(x, y, r, 0, Math.PI * 2);
  if (u.team === 0) {
    ctx.fillStyle = color;
    ctx.fill();
    ctx.strokeStyle = pal.onTeam;
    ctx.lineWidth = 1.2 * px;
    ctx.stroke();
  } else {
    ctx.fillStyle = pal.floor;
    ctx.fill();
    ctx.beginPath();
    ctx.arc(x, y, r * 0.84, 0, Math.PI * 2);
    ctx.strokeStyle = color;
    ctx.lineWidth = r * 0.32;
    ctx.stroke();
  }

  drawStateRings(pen, u, x, y, r);
  drawBars(pen, u, x, y, r);

  const teamSize = frame.units.length / 2;
  const slot = String(u.id - u.team * teamSize + 1);
  pen.inPixels(() => {
    const [sx, sy] = pen.screen(x, y);
    const size = Math.max(8, Math.min(18, r * camera.ppt * 1.15));
    ctx.font = `700 ${size}px ${FONT}`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillStyle = u.team === 0 ? pal.onTeam : color;
    ctx.fillText(slot, sx, sy + size * 0.06);
    if (camera.ppt >= 24) {
      const tags = stateTags(u);
      const text = `${weaponName(rules, u.weapon)} ${u.ammo}${tags.length ? " · " + tags.join(" ") : ""}`;
      ctx.font = `500 10px ${FONT}`;
      ctx.fillStyle = pal.dim;
      ctx.fillText(text, sx, sy + r * camera.ppt + 9);
    }
  });
}

/** Short state words for the label under a unit and for the side panel. */
export function stateTags(u: UnitFrame): string[] {
  const tags: string[] = [];
  if (u.state & S_WINDUP) tags.push("aiming");
  if (u.state & S_RELOADING) tags.push("reload");
  if (u.state & S_PLANTING) tags.push("planting");
  if (u.state & S_DEFUSING) tags.push("defusing");
  if (u.state & S_BLINDED) tags.push("blind");
  if (u.state & S_SLOWED) tags.push("slowed");
  if (u.state & S_REVEALED) tags.push("revealed");
  if (u.state & S_CASTING) tags.push("casting");
  return tags;
}

function drawStateRings(pen: Pen, u: UnitFrame, x: number, y: number, r: number): void {
  const { ctx, px } = pen;
  const { palette: pal, rules, t } = pen.scene;
  const ring = r + 4 * px;
  if (u.state & S_WINDUP) {
    ctx.beginPath();
    ctx.arc(x, y, ring, 0, Math.PI * 2);
    ctx.strokeStyle = pal.warn;
    ctx.lineWidth = 1.5 * px;
    ctx.stroke();
  }
  if (u.state & S_RELOADING) {
    // The reload length is a weapon rule the frame does not carry, so this is a spinner, not a gauge.
    const spin = (u.channel - t) * 1.3;
    ctx.beginPath();
    ctx.arc(x, y, ring, spin, spin + Math.PI * 1.2);
    ctx.strokeStyle = pal.dim;
    ctx.lineWidth = 2 * px;
    ctx.stroke();
  }
  if (u.state & (S_PLANTING | S_DEFUSING)) {
    const total = u.state & S_PLANTING ? rules.plant_ticks : rules.defuse_ticks;
    const done = Math.max(0, Math.min(1, 1 - (u.channel - t) / total));
    ctx.beginPath();
    ctx.arc(x, y, ring + 2 * px, -Math.PI / 2, -Math.PI / 2 + Math.PI * 2 * done);
    ctx.strokeStyle = pal.charge;
    ctx.lineWidth = 3 * px;
    ctx.stroke();
  }
  if (u.state & S_BLINDED) {
    ctx.beginPath();
    ctx.arc(x, y, ring, 0, Math.PI * 2);
    ctx.moveTo(x - ring * 0.7, y + ring * 0.7);
    ctx.lineTo(x + ring * 0.7, y - ring * 0.7);
    ctx.strokeStyle = pal.flash;
    ctx.lineWidth = 2 * px;
    ctx.setLineDash([3 * px, 3 * px]);
    ctx.stroke();
    ctx.setLineDash([]);
  }
  if (u.state & S_REVEALED) {
    ctx.beginPath();
    ctx.arc(x, y, ring + 4 * px, 0, Math.PI * 2);
    ctx.strokeStyle = alpha(pal.warn, 0.7);
    ctx.lineWidth = px;
    ctx.setLineDash([2 * px, 4 * px]);
    ctx.stroke();
    ctx.setLineDash([]);
  }
}

/** Health over the unit, with a thinner armour bar above it. */
function drawBars(pen: Pen, u: UnitFrame, x: number, y: number, r: number): void {
  const { ctx, px } = pen;
  const { palette: pal, rules } = pen.scene;
  const w = Math.max(r * 2.4, 18 * px);
  const h = 3 * px;
  const left = x - w / 2;
  const top = y - r - 9 * px;
  const hp = Math.max(0, Math.min(1, u.hp / rules.max_hp));
  ctx.fillStyle = pal.barBack;
  ctx.fillRect(left - px, top - px, w + 2 * px, h + 2 * px);
  ctx.fillStyle = hp < 0.35 ? pal.hpLow : pal.hp;
  ctx.fillRect(left, top, w * hp, h);
  if (u.armor > 0) {
    ctx.fillStyle = pal.armor;
    ctx.fillRect(left, top - 3 * px, w * Math.min(1, u.armor / 50), 2 * px);
  }
}
