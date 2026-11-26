// Combat lab page. The map, units, bullets and drops are drawn by the replay viewer (`render.ts`,
// `theme.ts`). This file only supplies the lab's frames and the fan marks those modules do not know about.

import { fitCamera, pan, zoomAt, type Camera } from "./camera";
import { stateTags, unitLabel } from "./draw-units";
import {
  damageAt,
  fanOf,
  mismatches,
  pelletAngle,
  polar,
  startupTicks,
  tilesOf,
  type Fan,
  type LabFile,
  type LabFrame,
  type LabScene,
  type Weapon,
} from "./lab-math";
import { parseMap, type GameMap } from "./map";
import { ALL, viewOf } from "./perspective";
import { FONT, render, type Pen } from "./render";
import { applyTheme, alpha, type Palette, type ThemeName } from "./theme";
import {
  DROP_WEAPON,
  EVENT_DAMAGE,
  PHASE_ACTION,
  S_ALIVE,
  S_RELOADING,
  S_WINDUP,
  TILE,
  type Frame,
  type UnitFrame,
} from "./types";
import { weaponName, type ViewRules } from "./vision";

const GROUPS: [string, string, string][] = [
  ["studio", "Weapon studio", "Fans, falloff and a held trigger. The units and the floor are the replay viewer's; the rays are the engine's fan()."],
  ["ttk", "Time to kill", "The duels in combat_ttk.rs. Step 0 is the setup. The cell's number is the step the target dies."],
  ["pull", "One trigger pull", "Wind-up, the tick the bullets leave, and cooldown."],
  ["fans", "Where the bullets go", "Movement on the release tick, recoil walking the group off the aim, and a shot clamped to the view cone."],
  ["flight", "Flight and damage", "Falloff, the end of range, walls, a dodge, and two lethal shots in one slice."],
  ["handling", "Reload, cancel, swap", "Reloading, a free feint, dying during the wind-up, and a swap that keeps the cooldown."],
  ["drops", "Weapons on the ground", "Death drops bought guns and not the sidearm. A gun on the ground is empty."],
];

const STUDIO_W = 46;
const STUDIO_H = 18;
const BASE_TPS = 8;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const canvas = $<HTMLCanvasElement>("canvas");
const ctx = canvas.getContext("2d")!;
const stage = $("stage");

let lab: LabFile | null = null;
let palette: Palette = applyTheme("dark");
let themeName: ThemeName = "dark";
let mode = "studio";
let scene: LabScene | null = null;
let index = 0;
let playing = false;
let acc = 0;
let lastTs = 0;
let camera: Camera = { cx: STUDIO_W / 2, cy: STUDIO_H / 2, ppt: 20 };
let cameraIsAuto = true;
let width = 1;
let height = 1;
let dpr = 1;
const maps = new Map<string, GameMap>();
let studioMap: GameMap | null = null;
const studio = { unsteady: 0, recoil: 0, tiles: 10, weapon: 1 };

function rulesOf(file: LabFile): ViewRules {
  return {
    unit_radius: file.radius,
    fov_half: file.fovHalf,
    vision_range: file.visionRange,
    sense_radius: file.senseRadius,
    max_hp: file.maxHp,
    plant_ticks: 12,
    defuse_ticks: 21,
    fuse_ticks: 80,
    weapons: file.weapons.map((w) => w.name),
  };
}

function mapOf(m: { id: string; w: number; h: number; tiles: string }): GameMap {
  const hit = maps.get(m.id);
  if (hit) return hit;
  const rows: string[] = [];
  for (let y = 0; y < m.h; y++) rows.push(m.tiles.slice(y * m.w, (y + 1) * m.w));
  const parsed = parseMap(`name: lab\nteam_size: 1\n---\n${rows.join("\n")}\n`);
  maps.set(m.id, parsed);
  return parsed;
}

function blankMap(): GameMap {
  if (studioMap) return studioMap;
  const row = ".".repeat(STUDIO_W);
  studioMap = parseMap(`name: studio\nteam_size: 1\n---\n${Array(STUDIO_H).fill(row).join("\n")}\n`);
  return studioMap;
}

function toUnit(row: number[], i: number): UnitFrame {
  let state = 0;
  if (row[4]) state |= S_ALIVE;
  if (row[10]) state |= S_WINDUP;
  if (row[14] > 0) state |= S_RELOADING;
  return {
    id: i,
    team: row[3],
    side: 0,
    kit: 0,
    alive: row[4] === 1,
    state,
    x: row[0],
    y: row[1],
    facing: row[2],
    vx: 0,
    vy: 0,
    hp: row[5],
    armor: row[6],
    weapon: row[7],
    ammo: row[8],
    channel: row[10] ? row[11] : row[14],
    aim: row[13],
    credits: 0,
    seen: "0",
    kills: 0,
    deaths: 0,
    sounds: [],
  };
}

function toFrame(f: LabFrame, tick: number): Frame {
  const units = f.u.map(toUnit);
  return {
    tick,
    round: 1,
    phase: PHASE_ACTION,
    phase_left: 0,
    score: [0, 0],
    attackers: 0,
    charge: { state: 0, x: 0, y: 0 },
    units,
    projectiles: f.b.map((b) => ({
      owner: b[5],
      team: units[b[5]]?.team ?? 0,
      weapon: b[2],
      kind: 0,
      x: b[0],
      y: b[1],
      vx: b[3],
      vy: b[4],
    })),
    effects: [],
    drops: f.d.map((d) => ({ kind: DROP_WEAPON, x: d[0], y: d[1], item: d[2] })),
    events: f.hit.map((h) => ({ kind: EVENT_DAMAGE, actor: h[0], target: h[1], x: 0, y: 0, value: h[2] })),
    debug: [],
    winner: null,
  };
}

function studioFrame(file: LabFile, gun: Weapon): Frame {
  const ox = 2 * TILE;
  const oy = 9 * TILE;
  const dist = studio.tiles * TILE;
  const shooter = toUnit([ox, oy, 0, 0, 1, file.maxHp, 0, gun.id, gun.magazine, studio.recoil, 0, 0, 0, 0, 0, 0, gun.id, gun.magazine, -1, 0, gun.slot], 0);
  const target = toUnit([ox + dist, oy, 32768, 1, 1, file.maxHp, 0, -1, 0, 0, 0, 0, 0, 32768, 0, 0, -1, 0, -1, 0, 1], 1);
  return {
    tick: 0,
    round: 1,
    phase: PHASE_ACTION,
    phase_left: 0,
    score: [0, 0],
    attackers: 0,
    charge: { state: 0, x: 0, y: 0 },
    units: [shooter, target],
    projectiles: [],
    effects: [],
    drops: [],
    events: [],
    debug: [],
    winner: null,
  };
}

function paintFan(pen: Pen, gun: Weapon, fan: Fan, ox: number, oy: number, ghost: boolean): void {
  const { ctx, px } = pen;
  const k = 1 / TILE;
  for (let i = 0; i < fan.pellets; i++) {
    const ang = pelletAngle(fan, i);
    const muzzle = polar(ang, lab!.radius);
    const steps = 12;
    for (let s = 0; s < steps; s++) {
      const d0 = Math.trunc((gun.range * s) / steps);
      const d1 = Math.trunc((gun.range * (s + 1)) / steps);
      const a = polar(ang, lab!.radius + d0);
      const b = polar(ang, lab!.radius + d1);
      const t = gun.damage ? damageAt(gun, d0) / gun.damage : 0;
      ctx.strokeStyle = ghost ? alpha(palette.team[0], 0.35) : mix(palette.dim, palette.fire, t);
      ctx.lineWidth = (ghost ? 1 : 1.6) * px;
      ctx.beginPath();
      ctx.moveTo((ox + muzzle[0] + (a[0] - muzzle[0])) * k, (oy + muzzle[1] + (a[1] - muzzle[1])) * k);
      ctx.lineTo((ox + b[0]) * k, (oy + b[1]) * k);
      ctx.stroke();
    }
  }
}

function mix(from: string, to: string, t: number): string {
  const a = Number.parseInt(from.slice(1), 16);
  const b = Number.parseInt(to.slice(1), 16);
  const ch = (shift: number) => Math.round(((a >> shift) & 255) + (((b >> shift) & 255) - ((a >> shift) & 255)) * t);
  return `rgb(${ch(16)}, ${ch(8)}, ${ch(0)})`;
}

function burstMarks(pen: Pen): void {
  if (!lab || !scene) return;
  const releases: number[] = [];
  for (let k = 1; k <= index; k++) {
    const prev = scene.frames[k - 1];
    const cur = scene.frames[k];
    if (cur.u.some((u, i) => u[15] > prev.u[i][15])) releases.push(k);
  }
  const shown = releases.slice(-4);
  const { ctx, px } = pen;
  const k = 1 / TILE;
  shown.forEach((frameIndex, n) => {
    const frame = scene!.frames[frameIndex];
    const fade = n === shown.length - 1 ? 0.85 : 0.3;
    for (const b of frame.b) {
      const owner = frame.u[b[5]];
      const gun = lab!.weapons.find((w) => w.id === b[2]);
      if (!owner || !gun) continue;
      const len = Math.hypot(b[3], b[4]) || 1;
      const x0 = owner[0] + (b[3] / len) * lab!.radius;
      const y0 = owner[1] + (b[4] / len) * lab!.radius;
      const x1 = x0 + (b[3] / len) * gun.range;
      const y1 = y0 + (b[4] / len) * gun.range;
      ctx.strokeStyle = alpha(palette.team[owner[3]] ?? palette.text, fade);
      ctx.lineWidth = 1.4 * px;
      ctx.beginPath();
      ctx.moveTo(x0 * k, y0 * k);
      ctx.lineTo(x1 * k, y1 * k);
      ctx.stroke();
    }
  });
}

function draw(): void {
  if (!lab) {
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.fillStyle = palette.bg;
    ctx.fillRect(0, 0, width, height);
    return;
  }
  const rules = rulesOf(lab);
  let frame: Frame;
  let map: GameMap;
  let marks: (pen: Pen) => void;
  if (mode === "studio") {
    const gun = lab.weapons.find((w) => w.id === studio.weapon) ?? lab.weapons[0];
    frame = studioFrame(lab, gun);
    map = blankMap();
    const ox = 2 * TILE;
    const oy = 9 * TILE;
    marks = (pen) => {
      paintFan(pen, gun, fanOf(gun, 0, 0, lab!.fovHalf, 0), ox, oy, true);
      paintFan(pen, gun, fanOf(gun, 0, 0, lab!.fovHalf, studio.recoil), ox, oy, false);
    };
  } else if (scene) {
    frame = toFrame(scene.frames[index], index);
    map = mapOf(lab.maps.find((m) => m.id === scene!.map)!);
    marks = burstMarks;
  } else return;
  const view = viewOf(frame, ALL, map, rules);
  render(ctx, width, height, dpr, {
    map,
    rules,
    frame,
    next: null,
    t: 0,
    view,
    camera,
    palette,
    show: { vision: true, debug: false, sounds: false },
    selected: null,
    marks,
  });
  if (mode === "studio") drawInset();
  if (scene && mode !== "studio") updateReadout();
}

function drawInset(): void {
  if (!lab) return;
  const gun = lab.weapons.find((w) => w.id === studio.weapon) ?? lab.weapons[0];
  const fan = fanOf(gun, 0, 0, lab.fovHalf, studio.recoil);
  const dist = studio.tiles * TILE;
  const covers = Math.trunc((lab.radius * 10430) / Math.max(1, dist));
  const size = 150;
  const x = width - size - 12;
  const y = height - size - 12;
  if (x < 8) return;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.fillStyle = alpha(palette.panel, 0.92);
  ctx.strokeStyle = palette.border;
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.roundRect(x, y, size, size, 8);
  ctx.fill();
  ctx.stroke();
  const cx = x + size / 2;
  const cy = y + size / 2 + 8;
  const scale = 40 / lab.radius;
  ctx.strokeStyle = palette.team[1];
  ctx.lineWidth = 2;
  ctx.beginPath();
  ctx.arc(cx, cy, 40, 0, Math.PI * 2);
  ctx.stroke();
  for (let i = 0; i < fan.pellets; i++) {
    const ang = pelletAngle(fan, i);
    const hit = Math.abs(angleDiffSafe(ang)) <= covers;
    const p = polar(ang, dist);
    const dir = polar(ang, 1024);
    const len = Math.hypot(dir[0], dir[1]) || 1;
    const ux = dir[0] / len;
    const uy = dir[1] / len;
    const lx = p[0] - dist;
    const ly = p[1];
    const span = lab.radius * 3;
    ctx.strokeStyle = hit ? palette.heal : alpha(palette.text, 0.45);
    ctx.beginPath();
    ctx.moveTo(cx + (lx - ux * span) * scale, cy + (ly - uy * span) * scale);
    ctx.lineTo(cx + (lx + ux * span) * scale, cy + (ly + uy * span) * scale);
    ctx.stroke();
  }
  ctx.fillStyle = palette.dim;
  ctx.font = `11px ${FONT}`;
  ctx.textAlign = "left";
  ctx.textBaseline = "alphabetic";
  ctx.fillText(`target at ${studio.tiles} tiles`, x + 10, y + 16);
}

function angleDiffSafe(angle: number): number {
  let d = angle & 0xffff;
  if (d >= 0x8000) d -= 0x10000;
  return d;
}

function fit(): void {
  if (!lab) return;
  if (mode === "studio") {
    camera = fitCamera(STUDIO_W, STUDIO_H, width, height);
    return;
  }
  if (!scene) return;
  const map = mapOf(lab.maps.find((m) => m.id === scene!.map)!);
  if (map.width <= 40 && map.height <= 40) {
    camera = fitCamera(map.width, map.height, width, height);
    return;
  }
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  for (const f of scene.frames) {
    for (const u of f.u) grow(u[0], u[1]);
    for (const b of f.b) grow(b[0], b[1]);
  }
  const pad = lab.tile * 2;
  const cam = fitCamera(Math.max(1, (x1 - x0 + pad * 2) / TILE), Math.max(1, (y1 - y0 + pad * 2) / TILE), width, height);
  camera = { ...cam, cx: (x0 - pad) / TILE + cam.cx, cy: (y0 - pad) / TILE + cam.cy };
  function grow(x: number, y: number): void {
    x0 = Math.min(x0, x);
    y0 = Math.min(y0, y);
    x1 = Math.max(x1, x);
    y1 = Math.max(y1, y);
  }
}

function resize(): void {
  const box = stage.getBoundingClientRect();
  width = Math.max(1, box.width);
  height = Math.max(1, box.height);
  dpr = window.devicePixelRatio || 1;
  canvas.width = Math.round(width * dpr);
  canvas.height = Math.round(height * dpr);
  if (cameraIsAuto) fit();
}

function setTheme(name: ThemeName): void {
  themeName = name;
  palette = applyTheme(name);
  $("btn-theme").textContent = name === "dark" ? "Light" : "Dark";
  try {
    localStorage.setItem("ibc-theme", name);
  } catch {
    // Private mode. The theme lasts for this page only.
  }
  paintCheck();
  renderPanel();
}

function load(file: LabFile): void {
  lab = file;
  maps.clear();
  studio.weapon = file.weapons.find((w) => w.name === "rifle")?.id ?? file.weapons[0].id;
  $("empty").hidden = true;
  $("match-title").textContent = "ruleset " + file.ruleset.slice(0, 12);
  paintCheck();
  selectGroup("studio");
}

function paintCheck(): void {
  if (!lab) return;
  const bad = mismatches(lab);
  const check = $("check");
  check.classList.toggle("bad", bad.length > 0);
  check.style.color = bad.length ? "" : palette.heal;
  check.textContent = bad.length
    ? `${bad.length} samples disagree with the engine`
    : `${lab.checks.length + lab.damageChecks.length} samples match the engine`;
}

function selectGroup(id: string): void {
  playing = false;
  mode = id;
  if (id === "studio") scene = null;
  else scene = lab?.scenes.find((s) => s.group === id) ?? null;
  index = 0;
  cameraIsAuto = true;
  renderNav();
  renderPanel();
  fit();
  syncTransport();
}

function selectScene(s: LabScene): void {
  scene = s;
  mode = s.group;
  index = 0;
  playing = false;
  cameraIsAuto = true;
  renderNav();
  renderPanel();
  fit();
  syncTransport();
}

function renderNav(): void {
  const nav = $("lab-nav");
  nav.replaceChildren();
  const h = document.createElement("h2");
  h.textContent = "Sections";
  nav.append(h);
  for (const [id, title] of GROUPS) {
    const b = document.createElement("button");
    b.type = "button";
    b.textContent = title;
    b.className = mode === id ? "on" : "";
    b.addEventListener("click", () => selectGroup(id));
    nav.append(b);
  }
  if (lab && mode !== "studio" && mode !== "ttk") {
    const h2 = document.createElement("h2");
    h2.textContent = "Scripts";
    nav.append(h2);
    for (const s of lab.scenes.filter((s) => s.group === mode)) {
      const b = document.createElement("button");
      b.type = "button";
      b.textContent = s.title;
      b.className = scene?.id === s.id ? "on" : "";
      b.addEventListener("click", () => selectScene(s));
      nav.append(b);
    }
  }
}

function renderPanel(): void {
  const panel = $("panel");
  panel.replaceChildren();
  if (!lab) return;
  if (mode === "studio") renderStudio(panel);
  else renderScenePanel(panel);
}

function renderStudio(panel: HTMLElement): void {
  const gun = lab!.weapons.find((w) => w.id === studio.weapon) ?? lab!.weapons[0];
  panel.append(heading(gun.name), paragraph(GROUPS[0][2]));
  const guns = document.createElement("div");
  guns.className = "weapons";
  for (const w of lab!.weapons) {
    const b = document.createElement("button");
    b.type = "button";
    b.textContent = w.name;
    b.className = w.id === gun.id ? "on" : "";
    b.addEventListener("click", () => {
      studio.weapon = w.id;
      studio.unsteady = Math.min(studio.unsteady, w.unsteadyMax);
      renderPanel();
    });
    guns.append(b);
  }
  panel.append(guns);
  slider(panel, "Unsteady", 0, gun.unsteadyMax, 1, studio.unsteady, (v) => (studio.unsteady = v));
  slider(panel, "Recoil", 0, 24, 1, studio.recoil, (v) => (studio.recoil = v));
  slider(panel, "Target", 1, 42, 1, studio.tiles, (v) => (studio.tiles = v));
  const table = document.createElement("table");
  table.id = "readouts";
  panel.append(table);
  refreshReadouts();
  panel.append(paragraph("Movement adds no spread; it lengthens the startup. Yellow is the startup at this unsteadiness, white the release, then cooldown, then the next braced pull."));
  const tl = document.createElement("div");
  tl.className = "timeline";
  const swatch: Record<string, string> = { wind: palette.warn, shot: palette.text, cool: palette.border, ready: palette.heal };
  for (const kind of pullTicks(gun, startupTicks(gun, studio.unsteady))) {
    const i = document.createElement("i");
    i.style.background = swatch[kind];
    tl.append(i);
  }
  panel.append(tl);
  panel.append(paragraph("Pellet step of each pull while holding the trigger braced. Measured in the engine."));
  const hold = document.createElement("canvas");
  hold.id = "hold";
  hold.className = "chart";
  panel.append(hold);
  panel.append(paragraph("Damage of one bullet by distance flown. The mark is the target."));
  const dmg = document.createElement("canvas");
  dmg.id = "dmg";
  dmg.className = "chart";
  panel.append(dmg);
  requestAnimationFrame(() => paintCharts(gun));
}

function pullTicks(w: Weapon, startup: number): string[] {
  const ticks: string[] = [];
  for (let i = 0; i < startup; i++) ticks.push("wind");
  ticks.push("shot");
  for (let i = 0; i < w.cooldown; i++) ticks.push("cool");
  ticks.push("ready");
  return ticks;
}

function studioRows(gun: Weapon): [string, string][] {
  const fan = fanOf(gun, 0, 0, lab!.fovHalf, studio.recoil);
  const dist = studio.tiles * lab!.tile;
  const covers = Math.trunc((lab!.radius * 10430) / Math.max(1, dist));
  const inner = fan.pellets % 2 === 0 ? Math.trunc(fan.step / 2) : 0;
  let onDisc = 0;
  for (let i = 0; i < fan.pellets; i++) if (Math.abs(angleDiffSafe(pelletAngle(fan, i))) <= covers) onDisc++;
  return [
    ["Slot", gun.slot === 1 ? "sidearm" : "primary"],
    ["Startup", String(startupTicks(gun, studio.unsteady))],
    ["Ticks per braced shot", String(1 + gun.cooldown)],
    ["Extra spread", String(fan.extra)],
    ["Pellet step", String(fan.step)],
    ["Full fan", String((gun.pellets - 1) * fan.step)],
    ["Close / at range", `${gun.damage * gun.pellets} / ${damageAt(gun, gun.range) * gun.pellets}`],
    ["Range", tilesOf(lab!.tile, gun.range) + " tiles"],
    ["Bullet", tilesOf(lab!.tile, gun.bulletSpeed) + " / tick"],
    ["Top speed", String(gun.topSpeed)],
    ["Target covers", covers + " angle"],
    ["Pellets on disc", `${onDisc} / ${fan.pellets}`],
    ["Duel fires", inner <= covers ? "yes" : "holds"],
  ];
}

function refreshReadouts(): void {
  const gun = lab?.weapons.find((w) => w.id === studio.weapon);
  const table = document.getElementById("readouts");
  if (!gun || !table) return;
  table.replaceChildren();
  for (const [k, v] of studioRows(gun)) {
    const tr = document.createElement("tr");
    const th = document.createElement("th");
    th.textContent = k;
    const td = document.createElement("td");
    td.textContent = v;
    tr.append(th, td);
    table.append(tr);
  }
  paintCharts(gun);
}

function slider(parent: HTMLElement, name: string, min: number, max: number, step: number, value: number, on: (v: number) => void): void {
  const labEl = document.createElement("label");
  labEl.className = "row";
  const span = document.createElement("span");
  span.textContent = name;
  const input = document.createElement("input");
  input.type = "range";
  input.min = String(min);
  input.max = String(Math.max(min, max));
  input.step = String(step);
  input.value = String(Math.min(Math.max(value, min), max));
  const out = document.createElement("output");
  out.textContent = input.value;
  input.addEventListener("input", () => {
    out.textContent = input.value;
    on(Number(input.value));
    refreshReadouts();
  });
  labEl.append(span, input, out);
  parent.append(labEl);
}

function paintCharts(gun: Weapon): void {
  const hold = document.getElementById("hold") as HTMLCanvasElement | null;
  const dmg = document.getElementById("dmg") as HTMLCanvasElement | null;
  if (hold) paintHold(hold, gun);
  if (dmg) paintDamage(dmg, gun);
}

function sized(canvasEl: HTMLCanvasElement): { w: number; h: number; g: CanvasRenderingContext2D } {
  const r = canvasEl.getBoundingClientRect();
  const ratio = window.devicePixelRatio || 1;
  canvasEl.width = Math.max(1, Math.floor(r.width * ratio));
  canvasEl.height = Math.max(1, Math.floor(r.height * ratio));
  const g = canvasEl.getContext("2d")!;
  g.setTransform(ratio, 0, 0, ratio, 0, 0);
  return { w: r.width, h: r.height, g };
}

function paintHold(canvasEl: HTMLCanvasElement, gun: Weapon): void {
  const { w, h, g } = sized(canvasEl);
  g.clearRect(0, 0, w, h);
  const samples = gun.hold;
  if (!samples.length) return;
  const most = Math.max(1, ...samples.map((s) => s[1]));
  const gap = w / samples.length;
  samples.forEach((s, i) => {
    const bh = (s[1] / most) * (h - 8);
    g.fillStyle = s[1] > gun.spread ? palette.warn : palette.border;
    g.fillRect(i * gap + 1, h - Math.max(1, bh), Math.max(1, gap - 2), Math.max(1, bh));
  });
}

function paintDamage(canvasEl: HTMLCanvasElement, gun: Weapon): void {
  const { w, h, g } = sized(canvasEl);
  g.clearRect(0, 0, w, h);
  g.beginPath();
  g.strokeStyle = palette.fire;
  g.lineWidth = 1.5;
  const n = 48;
  for (let i = 0; i <= n; i++) {
    const d = Math.trunc((gun.range * i) / n);
    const yy = h - 8 - (damageAt(gun, d) / gun.damage) * (h - 16);
    const xx = (d / gun.range) * (w - 8) + 4;
    if (i === 0) g.moveTo(xx, yy);
    else g.lineTo(xx, yy);
  }
  g.stroke();
  const mark = Math.min(gun.range, studio.tiles * lab!.tile);
  const mx = (mark / gun.range) * (w - 8) + 4;
  g.strokeStyle = palette.dim;
  g.beginPath();
  g.moveTo(mx, 4);
  g.lineTo(mx, h - 4);
  g.stroke();
}

function renderScenePanel(panel: HTMLElement): void {
  if (scene?.group === "ttk") ttkTable(panel);
  const info = GROUPS.find((g) => g[0] === mode);
  panel.append(heading(scene?.title ?? info?.[1] ?? ""), paragraph(scene?.note ?? info?.[2] ?? ""));
  if (scene) {
    const badge = document.createElement("div");
    badge.className = "badge";
    badge.id = "outcome";
    badge.textContent = scene.outcome;
    panel.append(badge);
    const ul = document.createElement("ul");
    ul.id = "units";
    panel.append(ul);
    const hits = document.createElement("div");
    hits.id = "hits";
    panel.append(hits);
    updateReadout();
  }
  panel.append(paragraph("Same units, walls and bullets as the replay viewer. Space plays, arrows step, drag pans, wheel zooms."));
}

function ttkTable(panel: HTMLElement): void {
  panel.append(paragraph(GROUPS.find((g) => g[0] === "ttk")![2]));
  for (const armor of [0, lab!.heavyArmor]) {
    panel.append(heading(armor === 0 ? "No armour" : `Heavy armour (${armor})`));
    const table = document.createElement("table");
    const head = document.createElement("tr");
    head.append(document.createElement("th"));
    for (const d of [5, 10, 20]) {
      for (const label of ["still", "ran"]) {
        const th = document.createElement("th");
        th.textContent = `${d} ${label}`;
        head.append(th);
      }
    }
    table.append(head);
    for (const w of lab!.weapons) {
      const tr = document.createElement("tr");
      const name = document.createElement("th");
      name.textContent = w.name;
      tr.append(name);
      for (const d of [5, 10, 20]) {
        for (const running of [false, true]) {
          const s = lab!.scenes.find((sc) => sc.ttk && sc.ttk.weapon === w.name && sc.ttk.tiles === d && sc.ttk.running === running && sc.ttk.armor === armor);
          const td = document.createElement("td");
          const b = document.createElement("button");
          b.type = "button";
          b.textContent = s && s.outcome === "no kill" ? "no" : (s?.outcome ?? "");
          b.title = s?.outcome ?? "";
          if (scene?.id === s?.id) b.className = "on";
          if (s) b.addEventListener("click", () => selectScene(s));
          td.append(b);
          tr.append(td);
        }
      }
      table.append(tr);
    }
    panel.append(table);
  }
}

function updateReadout(): void {
  if (!lab || !scene) return;
  const frame = toFrame(scene.frames[index], index);
  const teamSize = Math.max(1, Math.floor(frame.units.length / 2));
  const rules = rulesOf(lab);
  const ul = document.getElementById("units");
  if (ul) {
    ul.replaceChildren();
    for (const u of frame.units) {
      const li = document.createElement("li");
      const tags = stateTags(u);
      li.textContent = `${unitLabel(u, teamSize)} ${weaponName(rules, u.weapon)}  hp ${u.hp}  armour ${u.armor}  ammo ${u.ammo}${tags.length ? "  " + tags.join(" ") : ""}`;
      ul.append(li);
    }
  }
  const hits = document.getElementById("hits");
  const raw = scene.frames[index].hit;
  if (hits) {
    hits.textContent = raw.length
      ? raw.map((h) => `damage ${h[2]} on ${unitLabel(frame.units[h[1]], teamSize)}`).join("  ")
      : "";
  }
  $("readout").textContent = `step ${index} / ${scene.frames.length - 1}`;
  $<HTMLInputElement>("scrub").max = String(scene.frames.length - 1);
  $<HTMLInputElement>("scrub").value = String(index);
  $("btn-play").textContent = playing ? "Pause" : "Play";
  $("btn-jump").hidden = !(scene.ttk && /^\d+$/.test(scene.outcome));
}

function syncTransport(): void {
  const scrub = $<HTMLInputElement>("scrub");
  if (!scene || mode === "studio") {
    scrub.max = "0";
    scrub.value = "0";
    $("readout").textContent = mode === "studio" ? "studio" : "no lab";
    $("btn-jump").hidden = true;
    $("btn-play").textContent = "Play";
    return;
  }
  updateReadout();
}

function heading(text: string): HTMLElement {
  const h = document.createElement("h2");
  h.textContent = text;
  return h;
}

function paragraph(text: string): HTMLElement {
  const p = document.createElement("p");
  p.textContent = text;
  return p;
}

function stepBy(delta: number): void {
  if (!scene) return;
  playing = false;
  index = Math.max(0, Math.min(scene.frames.length - 1, index + delta));
  syncTransport();
}

function frame(ts: number): void {
  if (playing && scene) {
    const speed = Number($<HTMLSelectElement>("speed").value);
    const dt = lastTs ? ts - lastTs : 0;
    lastTs = ts;
    acc += (dt * speed * BASE_TPS) / 1000;
    let moved = false;
    while (acc >= 1 && index < scene.frames.length - 1) {
      index++;
      acc -= 1;
      moved = true;
    }
    if (index >= scene.frames.length - 1) playing = false;
    if (moved || !playing) syncTransport();
  } else lastTs = ts;
  draw();
  requestAnimationFrame(frame);
}

$("btn-play").addEventListener("click", () => {
  if (!scene) return;
  if (index >= scene.frames.length - 1) index = 0;
  playing = !playing;
  acc = 0;
  lastTs = 0;
  $("btn-play").textContent = playing ? "Pause" : "Play";
});
$("btn-step-back").addEventListener("click", () => stepBy(-1));
$("btn-step").addEventListener("click", () => stepBy(1));
$("btn-jump").addEventListener("click", () => {
  if (scene && /^\d+$/.test(scene.outcome)) {
    playing = false;
    index = Math.min(scene.frames.length - 1, Number(scene.outcome));
    syncTransport();
  }
});
$("btn-fit").addEventListener("click", () => {
  cameraIsAuto = true;
  fit();
});
$<HTMLInputElement>("scrub").addEventListener("input", (e) => {
  playing = false;
  index = Number((e.target as HTMLInputElement).value);
  syncTransport();
});

function openFile(file: File): void {
  file.text().then((text) => {
    try {
      load(JSON.parse(text) as LabFile);
      setStatus("");
    } catch (err) {
      setStatus(err instanceof Error ? err.message : String(err), true);
    }
  });
}

function setStatus(text: string, error = false): void {
  const status = $("status");
  status.textContent = text;
  status.classList.toggle("error", error);
}

function choose(): void {
  $<HTMLInputElement>("file").click();
}
$("btn-open").addEventListener("click", choose);
$("btn-open-2").addEventListener("click", choose);
$<HTMLInputElement>("file").addEventListener("change", () => {
  const file = $<HTMLInputElement>("file").files?.[0];
  if (file) openFile(file);
});
$("btn-theme").addEventListener("click", () => setTheme(themeName === "dark" ? "light" : "dark"));

stage.addEventListener("dragover", (e) => {
  e.preventDefault();
  stage.classList.add("dragover");
});
stage.addEventListener("dragleave", () => stage.classList.remove("dragover"));
stage.addEventListener("drop", (e) => {
  e.preventDefault();
  stage.classList.remove("dragover");
  const file = e.dataTransfer?.files?.[0];
  if (file) openFile(file);
});

canvas.addEventListener("pointerdown", (e) => {
  canvas.setPointerCapture(e.pointerId);
  canvas.classList.add("panning");
  canvas.dataset.x = String(e.clientX);
  canvas.dataset.y = String(e.clientY);
});
canvas.addEventListener("pointermove", (e) => {
  if (!canvas.classList.contains("panning")) return;
  camera = pan(camera, e.clientX - Number(canvas.dataset.x), e.clientY - Number(canvas.dataset.y));
  canvas.dataset.x = String(e.clientX);
  canvas.dataset.y = String(e.clientY);
  cameraIsAuto = false;
});
canvas.addEventListener("pointerup", () => canvas.classList.remove("panning"));
canvas.addEventListener("wheel", (e) => {
  e.preventDefault();
  const rect = canvas.getBoundingClientRect();
  camera = zoomAt(camera, e.deltaY < 0 ? 1.12 : 1 / 1.12, e.clientX - rect.left, e.clientY - rect.top, width, height);
  cameraIsAuto = false;
}, { passive: false });
canvas.addEventListener("dblclick", () => {
  cameraIsAuto = true;
  fit();
});

window.addEventListener("keydown", (e) => {
  const tag = document.activeElement?.tagName;
  if (tag === "INPUT" || tag === "SELECT" || tag === "TEXTAREA") return;
  if (e.key === " ") {
    e.preventDefault();
    $("btn-play").click();
  } else if (e.key === "ArrowLeft") stepBy(e.shiftKey ? -10 : -1);
  else if (e.key === "ArrowRight") stepBy(e.shiftKey ? 10 : 1);
  else if (e.key === "f") {
    cameraIsAuto = true;
    fit();
  } else if (e.key === "t") setTheme(themeName === "dark" ? "light" : "dark");
  else if (e.key === "o") choose();
});

new ResizeObserver(() => resize()).observe(stage);

try {
  const saved = localStorage.getItem("ibc-theme");
  if (saved === "light" || saved === "dark") setTheme(saved);
} catch {
  // Ignore.
}

const query = new URLSearchParams(location.search).get("lab");
const initial = query ? fetch(query) : fetch("lab.json");
initial
  .then((r) => (r.ok ? r.json() : Promise.reject(new Error(query ? `could not load ${query}` : "no lab.json beside this page"))))
  .then((data) => load(data as LabFile))
  .catch(() => {
    renderNav();
    setStatus("");
  });

requestAnimationFrame(frame);
