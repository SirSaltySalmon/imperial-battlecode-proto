// Frames the lab draws through the replay viewer's renderer: the engine frames recorded for a script, and the
// still picture of the weapon studio.

import type { LabFile, LabScene, Weapon } from "./lab-math";
import { EVENT_DAMAGE, PHASE_ACTION, S_ALIVE, type EventFrame, type Frame, type UnitFrame } from "./types";

/** Shooter at (ox, oy) looking along +x with `gun`, an unarmed target `dist` further on facing back. */
export function studioFrame(file: LabFile, gun: Weapon, ox: number, oy: number, dist: number): Frame {
  const unit = (id: number, x: number, facing: number, weapon: number, ammo: number): UnitFrame => ({
    id,
    team: id,
    side: 0,
    kit: 0,
    alive: true,
    state: S_ALIVE,
    x,
    y: oy,
    facing,
    vx: 0,
    vy: 0,
    hp: file.maxHp,
    armor: 0,
    weapon,
    ammo,
    channel: 0,
    aim: facing,
    credits: 0,
    seen: "3",
    kills: 0,
    deaths: 0,
    sounds: [],
  });
  return {
    tick: 0,
    round: 1,
    phase: PHASE_ACTION,
    phase_left: 0,
    score: [0, 0],
    attackers: 0,
    charge: { state: 0, x: 0, y: 0 },
    units: [unit(0, ox, 0, gun.id, gun.magazine), unit(1, ox + dist, 32768, -1, 0)],
    projectiles: [],
    effects: [],
    drops: [],
    events: [],
    debug: [],
    winner: null,
  };
}

/** Frame indices up to `upTo` on which some unit released a pull (its shot counter rose). */
export function releases(scene: LabScene, upTo: number): number[] {
  const out: number[] = [];
  for (let k = 1; k <= upTo; k++) {
    const prev = scene.frames[k - 1].x;
    if (scene.frames[k].x.some((x, i) => x[3] > prev[i][3])) out.push(k);
  }
  return out;
}

/** Bounding box of every unit and bullet position in the script: `[x0, y0, x1, y1]`. */
export function sceneBounds(scene: LabScene): [number, number, number, number] {
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  for (const { frame } of scene.frames) {
    for (const p of [...frame.units, ...frame.projectiles]) {
      x0 = Math.min(x0, p.x);
      y0 = Math.min(y0, p.y);
      x1 = Math.max(x1, p.x);
      y1 = Math.max(y1, p.y);
    }
  }
  return [x0, y0, x1, y1];
}

export function damageEvents(frame: Frame): EventFrame[] {
  return frame.events.filter((e) => e.kind === EVENT_DAMAGE);
}
