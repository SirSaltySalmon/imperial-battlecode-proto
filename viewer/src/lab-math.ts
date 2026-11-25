// Integer fan and falloff, matching `crates/engine/src/combat`. Drawing colours come from theme.ts, not from here.

export interface Weapon {
  name: string;
  id: number;
  slot: number;
  damage: number;
  pellets: number;
  spread: number;
  moveSpread: number;
  recoilSpread: number;
  recoilPerShot: number;
  recoilRecovery: number;
  windup: number;
  cooldown: number;
  magazine: number;
  reloadTicks: number;
  bulletSpeed: number;
  range: number;
  falloffStart: number;
  falloffMinPct: number;
  movePct: number;
  topSpeed: number;
  /** Heat, sideways shift, fan step at each release while holding the trigger. */
  hold: [number, number, number][];
}

export interface LabMap {
  id: string;
  w: number;
  h: number;
  tiles: string;
}

export interface TtkKey {
  weapon: string;
  tiles: number;
  running: boolean;
  armor: number;
}

export interface LabFrame {
  u: number[][];
  b: number[][];
  d: number[][];
  hit: number[][];
}

export interface LabScene {
  id: string;
  group: string;
  title: string;
  note: string;
  outcome: string;
  map: string;
  ttk?: TtkKey;
  frames: LabFrame[];
}

export interface FanCheck {
  weapon: number;
  facing: number;
  aim: number;
  speed: number;
  recoil: number;
  shots: number;
  centre: number;
  step: number;
}

export interface DamageCheck {
  weapon: number;
  travelled: number;
  damage: number;
}

export interface LabFile {
  tile: number;
  radius: number;
  fovHalf: number;
  maxHp: number;
  runSpeed: number;
  heavyArmor: number;
  visionRange: number;
  senseRadius: number;
  ruleset: string;
  weapons: Weapon[];
  maps: LabMap[];
  scenes: LabScene[];
  checks: FanCheck[];
  damageChecks: DamageCheck[];
}

export interface Fan {
  centre: number;
  step: number;
  pellets: number;
  extra: number;
  bias: number;
}

export function angleDiff(a: number, b: number): number {
  let d = (b - a) & 0xffff;
  if (d >= 0x8000) d -= 0x10000;
  return d;
}

/** Bhaskara sine, the same approximation as `fixed::sin`, scaled by 16384. */
export function sinOne(angle: number): number {
  const a = angle & 0xffff;
  const x = a & 0x7fff;
  const p = x * (32768 - x);
  const v = Math.trunc((16 * p * 16384) / (5 * 1073741824 - 4 * p));
  return a >= 32768 ? -v : v;
}

export function polar(angle: number, len: number): [number, number] {
  return [Math.trunc((sinOne(angle + 16384) * len) / 16384), Math.trunc((sinOne(angle) * len) / 16384)];
}

export function extraSpread(w: Weapon, speed: number, recoil: number): number {
  const e = Math.trunc((w.moveSpread * Math.max(0, speed)) / 1024) + w.recoilSpread * Math.max(0, recoil);
  return Math.max(0, Math.min(32768, e));
}

export function fanOf(w: Weapon, facing: number, aim: number, fov: number, speed: number, recoil: number, shots: number): Fan {
  const extra = extraSpread(w, speed, recoil);
  const bias = shots % 2 === 0 ? Math.trunc(extra / 2) : -Math.trunc(extra / 2);
  let off = angleDiff(facing, aim);
  if (off > fov) off = fov;
  if (off < -fov) off = -fov;
  return { centre: (facing + off + bias) & 0xffff, step: w.spread + extra, pellets: w.pellets, extra, bias };
}

export function pelletAngle(fan: Fan, i: number): number {
  return (fan.centre + Math.trunc(((2 * i - (fan.pellets - 1)) * fan.step) / 2)) & 0xffff;
}

export function damageAt(w: Weapon, travelled: number): number {
  if (travelled <= w.falloffStart || w.range <= w.falloffStart) return w.damage;
  const span = w.range - w.falloffStart;
  const over = Math.min(travelled - w.falloffStart, span);
  const pct = 100 - Math.trunc(((100 - w.falloffMinPct) * over) / span);
  return Math.trunc((w.damage * pct) / 100);
}

export function tilesOf(tile: number, v: number): string {
  return Math.trunc(v / tile) + "." + Math.trunc(((v % tile) * 10) / tile);
}

/** Samples the engine recorded. Empty when the page's formulas still match. */
export function mismatches(lab: LabFile): unknown[] {
  const byId = new Map(lab.weapons.map((w) => [w.id, w]));
  const bad: unknown[] = [];
  for (const c of lab.checks) {
    const f = fanOf(byId.get(c.weapon)!, c.facing, c.aim, lab.fovHalf, c.speed, c.recoil, c.shots);
    if (f.centre !== c.centre || f.step !== c.step) bad.push(c);
  }
  for (const c of lab.damageChecks) {
    if (damageAt(byId.get(c.weapon)!, c.travelled) !== c.damage) bad.push(c);
  }
  return bad;
}
