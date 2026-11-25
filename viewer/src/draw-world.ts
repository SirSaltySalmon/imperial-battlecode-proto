// Everything on the map that is not a unit: effects, drops, the charge, bullets, sounds, debug drawings.

import type { Pose, Shot } from "./interp";
import { FONT, type Pen } from "./render";
import { alpha, debugColor } from "./theme";
import {
  CHARGE_CARRIED,
  CHARGE_DROPPED,
  CHARGE_PLANTED,
  DEBUG_DOT,
  DEBUG_LINE,
  DROP_CHARGE,
  EFFECT_BARRIER,
  EFFECT_DECOY,
  EFFECT_FIRE,
  EFFECT_FLASH,
  EFFECT_HEAL,
  EFFECT_RECON,
  EFFECT_SMOKE,
  PHASE_PLANTED,
  S_HAS_CHARGE,
  SOUND_SHOT,
  SOUND_STEP,
  TILE,
  type DebugDraw,
  type DropFrame,
  type EffectFrame,
} from "./types";
import { weaponName } from "./vision";

const K = 1 / TILE;

function circle(ctx: CanvasRenderingContext2D, x: number, y: number, r: number): void {
  ctx.beginPath();
  ctx.arc(x, y, Math.max(r, 0), 0, Math.PI * 2);
}

function label(pen: Pen, text: string, tx: number, ty: number, color: string, size = 10): void {
  pen.inPixels(() => {
    const [sx, sy] = pen.screen(tx, ty);
    pen.ctx.font = `600 ${size}px ${FONT}`;
    pen.ctx.textAlign = "center";
    pen.ctx.textBaseline = "middle";
    pen.ctx.fillStyle = color;
    pen.ctx.fillText(text, sx, sy);
  });
}

export function drawEffects(pen: Pen, effects: EffectFrame[]): void {
  const { ctx, px } = pen;
  const pal = pen.scene.palette;
  for (const e of effects) {
    const x = e.x * K;
    const y = e.y * K;
    const r = Math.max(e.radius * K, 0.2);
    // Effects fade out over their last ticks.
    const life = Math.min(1, (e.ticks_left + 1) / 4);
    const team = pal.team[e.team] ?? pal.dim;
    ctx.save();
    switch (e.kind) {
      case EFFECT_SMOKE:
        ctx.fillStyle = alpha(pal.smoke, 0.5 * life);
        for (const [ox, oy, s] of [[0, 0, 1], [0.35, 0.2, 0.7], [-0.3, 0.25, 0.75], [0.05, -0.38, 0.65]]) {
          circle(ctx, x + ox * r, y + oy * r, r * s);
          ctx.fill();
        }
        circle(ctx, x, y, r);
        ctx.strokeStyle = alpha(pal.smoke, 0.8 * life);
        ctx.lineWidth = 1.5 * px;
        ctx.stroke();
        break;
      case EFFECT_FIRE:
        circle(ctx, x, y, r);
        ctx.fillStyle = alpha(pal.fire, 0.22 * life);
        ctx.fill();
        ctx.strokeStyle = alpha(pal.fire, 0.95 * life);
        ctx.lineWidth = 2.5 * px;
        ctx.setLineDash([2 * px, 5 * px]);
        ctx.stroke();
        circle(ctx, x, y, r * 0.55);
        ctx.stroke();
        break;
      case EFFECT_BARRIER:
        ctx.beginPath();
        ctx.moveTo(x, y);
        ctx.lineTo(e.x2 * K, e.y2 * K);
        ctx.strokeStyle = alpha(team, life);
        ctx.lineWidth = Math.max(0.22, 5 * px);
        ctx.lineCap = "butt";
        ctx.stroke();
        ctx.strokeStyle = alpha(pal.text, 0.8 * life);
        ctx.lineWidth = 1.5 * px;
        ctx.stroke();
        break;
      case EFFECT_FLASH:
        circle(ctx, x, y, r);
        ctx.fillStyle = alpha(pal.flash, 0.28 * life);
        ctx.fill();
        ctx.strokeStyle = alpha(pal.flash, 0.9 * life);
        ctx.lineWidth = 1.5 * px;
        ctx.beginPath();
        for (let i = 0; i < 8; i++) {
          const a = (i / 8) * Math.PI * 2;
          ctx.moveTo(x + Math.cos(a) * r * 0.25, y + Math.sin(a) * r * 0.25);
          ctx.lineTo(x + Math.cos(a) * r, y + Math.sin(a) * r);
        }
        ctx.stroke();
        break;
      case EFFECT_RECON:
        circle(ctx, x, y, r);
        ctx.strokeStyle = alpha(team, 0.85 * life);
        ctx.lineWidth = 1.5 * px;
        ctx.setLineDash([6 * px, 4 * px]);
        ctx.stroke();
        ctx.setLineDash([]);
        circle(ctx, x, y, 3 * px);
        ctx.fillStyle = alpha(team, life);
        ctx.fill();
        break;
      case EFFECT_HEAL:
        circle(ctx, x, y, r);
        ctx.fillStyle = alpha(pal.heal, 0.18 * life);
        ctx.fill();
        ctx.strokeStyle = alpha(pal.heal, 0.9 * life);
        ctx.lineWidth = 1.5 * px;
        ctx.stroke();
        ctx.lineWidth = 3 * px;
        ctx.beginPath();
        ctx.moveTo(x - 6 * px, y);
        ctx.lineTo(x + 6 * px, y);
        ctx.moveTo(x, y - 6 * px);
        ctx.lineTo(x, y + 6 * px);
        ctx.stroke();
        break;
      case EFFECT_DECOY:
        circle(ctx, x, y, pen.scene.rules.unit_radius * K);
        ctx.strokeStyle = alpha(team, 0.9 * life);
        ctx.lineWidth = 1.5 * px;
        ctx.setLineDash([3 * px, 3 * px]);
        ctx.stroke();
        label(pen, "?", x, y, alpha(team, life));
        break;
      default:
        circle(ctx, x, y, r);
        ctx.strokeStyle = alpha(pal.dim, 0.8 * life);
        ctx.lineWidth = px;
        ctx.setLineDash([2 * px, 3 * px]);
        ctx.stroke();
        label(pen, `effect ${e.kind}`, x, y, pal.dim);
    }
    ctx.restore();
  }
}

export function drawDrops(pen: Pen, drops: DropFrame[]): void {
  const { ctx, px } = pen;
  const pal = pen.scene.palette;
  for (const d of drops) {
    // The dropped charge is drawn from `frame.charge` (see drawCharge).
    if (d.kind === DROP_CHARGE) continue;
    const x = d.x * K;
    const y = d.y * K;
    const s = Math.max(0.16, 5 * px);
    ctx.fillStyle = pal.panel2;
    ctx.strokeStyle = pal.dim;
    ctx.lineWidth = 1.5 * px;
    ctx.beginPath();
    ctx.rect(x - s, y - s * 0.6, s * 2, s * 1.2);
    ctx.fill();
    ctx.stroke();
    if (pen.scene.camera.ppt >= 22) label(pen, weaponName(pen.scene.rules, d.item), x, y + s + 7 * px, pal.dim, 9);
  }
}

function diamond(ctx: CanvasRenderingContext2D, x: number, y: number, s: number): void {
  ctx.beginPath();
  ctx.moveTo(x, y - s);
  ctx.lineTo(x + s, y);
  ctx.lineTo(x, y + s);
  ctx.lineTo(x - s, y);
  ctx.closePath();
}

/** The charge: a diamond on its carrier, on the ground, or planted with its fuse running down. */
export function drawCharge(pen: Pen, poses: Pose[]): void {
  const { ctx, px } = pen;
  const { frame, view, palette: pal, rules } = pen.scene;
  if (!view.chargeVisible) return;
  const size = Math.max(0.2, 5 * px);
  ctx.lineWidth = 1.5 * px;
  ctx.strokeStyle = pal.onTeam;
  ctx.fillStyle = pal.charge;
  if (frame.charge.state === CHARGE_CARRIED) {
    const i = frame.units.findIndex((u) => u.alive && u.state & S_HAS_CHARGE);
    if (i < 0) return;
    const r = rules.unit_radius * K;
    diamond(ctx, poses[i].x * K + r * 1.05, poses[i].y * K + r * 1.05, size * 0.8);
    ctx.fill();
    ctx.stroke();
    return;
  }
  const x = frame.charge.x * K;
  const y = frame.charge.y * K;
  if (frame.charge.state === CHARGE_PLANTED) {
    const left = frame.phase === PHASE_PLANTED ? Math.max(0, frame.phase_left - pen.scene.t) : 0;
    const frac = Math.min(1, left / rules.fuse_ticks);
    // The pulse quickens as the fuse runs down.
    const pulse = (left * (1 + (1 - frac) * 2)) % 1;
    circle(ctx, x, y, size * (1.6 + 2.2 * (1 - pulse)));
    ctx.strokeStyle = alpha(pal.charge, 0.15 + 0.5 * pulse);
    ctx.lineWidth = 2 * px;
    ctx.stroke();
    ctx.beginPath();
    ctx.arc(x, y, size * 1.9, -Math.PI / 2, -Math.PI / 2 + Math.PI * 2 * frac);
    ctx.strokeStyle = pal.charge;
    ctx.lineWidth = 3 * px;
    ctx.stroke();
    if (frame.phase === PHASE_PLANTED) label(pen, String(Math.ceil(left)), x, y - size * 1.9 - 9 * px, pal.charge, 11);
  } else if (frame.charge.state === CHARGE_DROPPED) {
    circle(ctx, x, y, size * 1.8);
    ctx.strokeStyle = alpha(pal.charge, 0.6);
    ctx.setLineDash([3 * px, 3 * px]);
    ctx.stroke();
    ctx.setLineDash([]);
  } else {
    return;
  }
  ctx.strokeStyle = pal.onTeam;
  ctx.lineWidth = 1.5 * px;
  diamond(ctx, x, y, size);
  ctx.fill();
  ctx.stroke();
}

export function drawShots(pen: Pen, shots: Shot[]): void {
  const { ctx, px } = pen;
  const pal = pen.scene.palette;
  for (const s of shots) {
    const [x, y, tx, ty] = [s.x * K, s.y * K, s.tx * K, s.ty * K];
    const color = pal.team[s.team] ?? pal.text;
    if (s.kind !== 0) {
      // A thrown ability device (placeholder look): a team-coloured ball with a white rim, no trail.
      circle(ctx, x, y, 5 * px);
      ctx.fillStyle = alpha(color, 0.9);
      ctx.fill();
      ctx.strokeStyle = pal.text;
      ctx.lineWidth = 1.5 * px;
      ctx.stroke();
      continue;
    }
    if (x !== tx || y !== ty) {
      const g = ctx.createLinearGradient(tx, ty, x, y);
      g.addColorStop(0, alpha(color, 0));
      g.addColorStop(1, alpha(color, 0.9));
      ctx.strokeStyle = g;
      ctx.lineWidth = 2 * px;
      ctx.beginPath();
      ctx.moveTo(tx, ty);
      ctx.lineTo(x, y);
      ctx.stroke();
    }
    circle(ctx, x, y, 2.2 * px);
    ctx.fillStyle = pal.text;
    ctx.fill();
  }
}

/** Sounds heard during the tick: a ring that spreads and fades over the tick. Shots are larger than steps. */
export function drawSounds(pen: Pen, sounds: [number, number, number][]): void {
  const { ctx, px } = pen;
  const pal = pen.scene.palette;
  const t = pen.scene.t;
  for (const [kind, sx, sy] of sounds) {
    const base = kind === SOUND_STEP ? 0.22 : kind === SOUND_SHOT ? 0.4 : 0.55;
    const rings = kind === SOUND_STEP ? 1 : 2;
    for (let i = 0; i < rings; i++) {
      circle(ctx, sx * K, sy * K, base * (1 + i * 0.6) * (0.7 + 0.8 * t));
      ctx.strokeStyle = alpha(pal.sound, (kind === SOUND_STEP ? 0.4 : 0.6) * (1 - 0.7 * t));
      ctx.lineWidth = (kind === SOUND_STEP ? 1 : 1.5) * px;
      ctx.setLineDash(kind === SOUND_STEP || kind === SOUND_SHOT ? [] : [3 * px, 3 * px]);
      ctx.stroke();
    }
  }
  ctx.setLineDash([]);
}

export function drawDebug(pen: Pen, debug: DebugDraw[]): void {
  const { ctx, px } = pen;
  ctx.lineWidth = 1.5 * px;
  for (const [, kind, x0, y0, x1, y1, rgb] of debug) {
    const color = debugColor(rgb);
    if (kind === DEBUG_LINE) {
      ctx.strokeStyle = color;
      ctx.beginPath();
      ctx.moveTo(x0 * K, y0 * K);
      ctx.lineTo(x1 * K, y1 * K);
      ctx.stroke();
    } else if (kind === DEBUG_DOT) {
      circle(ctx, x0 * K, y0 * K, x1 > 0 ? x1 * K : 3 * px);
      ctx.fillStyle = color;
      ctx.fill();
    }
  }
}
