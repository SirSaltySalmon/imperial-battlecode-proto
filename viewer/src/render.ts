// Draws one moment of the match on a 2D canvas. Geometry is drawn in tile units under a camera transform;
// text is drawn in screen pixels so it stays crisp at any zoom.

import type { Camera } from "./camera";
import { drawCharge, drawDebug, drawDrops, drawEffects, drawShots, drawSounds } from "./draw-world";
import { drawUnits } from "./draw-units";
import { closestOnSegment, interpShots, interpUnits, type Pose } from "./interp";
import { segmentWall, siteName, type GameMap } from "./map";
import type { View } from "./perspective";
import { alpha, type Palette } from "./theme";
import { EVENT_DAMAGE, S_BLINDED, TILE, TILE_LOW, TILE_WALL, type Frame, type ProjectileFrame, type Vec2 } from "./types";
import { visionPolygon, type ViewRules } from "./vision";

export interface Scene {
  map: GameMap;
  rules: ViewRules;
  frame: Frame;
  /** The following frame and how far playback is towards it (0..1); null / 0 when paused on a tick. */
  next: Frame | null;
  t: number;
  view: View;
  camera: Camera;
  palette: Palette;
  show: { vision: boolean; debug: boolean; sounds: boolean };
  /** Unit id to ring (the single-unit perspective), or null. */
  selected: number | null;
}

/** Shared by the drawing modules: the context plus the numbers they all need. */
export interface Pen {
  ctx: CanvasRenderingContext2D;
  scene: Scene;
  /** One CSS pixel in tile units. */
  px: number;
  /** Tile coordinates -> CSS pixels. */
  screen(tx: number, ty: number): Vec2;
  /** Run `draw` with the transform reset to CSS pixels (for text). */
  inPixels(draw: () => void): void;
}

interface MapPaths {
  walls: Path2D;
  low: Path2D;
  lowMarks: Path2D;
}

const mapPaths = new WeakMap<GameMap, MapPaths>();

function pathsFor(map: GameMap): MapPaths {
  let p = mapPaths.get(map);
  if (p) return p;
  p = { walls: new Path2D(), low: new Path2D(), lowMarks: new Path2D() };
  for (let ty = 0; ty < map.height; ty++) {
    for (let tx = 0; tx < map.width; tx++) {
      const tile = map.tiles[ty * map.width + tx];
      if (tile === TILE_WALL) {
        // Merge a horizontal run into one rectangle.
        let end = tx;
        while (end + 1 < map.width && map.tiles[ty * map.width + end + 1] === TILE_WALL) end++;
        p.walls.rect(tx, ty, end - tx + 1, 1);
        tx = end;
      } else if (tile === TILE_LOW) {
        p.low.rect(tx + 0.1, ty + 0.1, 0.8, 0.8);
        p.lowMarks.moveTo(tx + 0.1, ty + 0.9);
        p.lowMarks.lineTo(tx + 0.9, ty + 0.1);
      }
    }
  }
  mapPaths.set(map, p);
  return p;
}

let fogCanvas: HTMLCanvasElement | null = null;

export function render(ctx: CanvasRenderingContext2D, width: number, height: number, dpr: number, scene: Scene): void {
  const { camera: cam, palette: pal, frame, view } = scene;
  const ox = width / 2 - cam.cx * cam.ppt;
  const oy = height / 2 - cam.cy * cam.ppt;
  const toTiles = () => ctx.setTransform(dpr * cam.ppt, 0, 0, dpr * cam.ppt, dpr * ox, dpr * oy);
  const pen: Pen = {
    ctx,
    scene,
    px: 1 / cam.ppt,
    screen: (tx, ty) => [ox + tx * cam.ppt, oy + ty * cam.ppt],
    inPixels(draw) {
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      draw();
      toTiles();
    },
  };

  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.fillStyle = pal.bg;
  ctx.fillRect(0, 0, width, height);
  toTiles();
  ctx.lineCap = "round";
  ctx.lineJoin = "round";

  drawFloor(pen);
  const poses = interpUnits(frame.units, scene.next?.units ?? null, scene.t);
  const cones = visionCones(scene, poses);
  if (view.viewerIds.length > 0) drawFog(pen, cones, width, height, dpr, toTiles);
  else if (scene.show.vision) drawCones(pen, cones);
  drawWalls(pen);

  drawEffects(pen, view.effects);
  drawDrops(pen, view.drops);
  drawCharge(pen, poses);
  if (scene.show.sounds) drawSounds(pen, view.sounds);
  // Bullets are matched across the two frames on the full lists and fogged afterwards, by where each one
  // is drawn: a bullet about to enter the viewers' sight must not be traced back into the fog.
  const shots = interpShots(frame.projectiles, scene.next?.projectiles ?? null, scene.t, stopFn(scene));
  drawShots(pen, shots.filter((s) => s.team === view.team || view.canSee(s.x, s.y)));
  drawUnits(pen, poses);
  if (scene.show.debug) drawDebug(pen, view.debug);
}

function drawFloor(pen: Pen): void {
  const { ctx, px } = pen;
  const { map, palette: pal, frame, camera } = pen.scene;
  ctx.fillStyle = pal.floor;
  ctx.fillRect(0, 0, map.width, map.height);
  if (camera.ppt >= 12) {
    ctx.beginPath();
    for (let x = 1; x < map.width; x++) {
      ctx.moveTo(x, 0);
      ctx.lineTo(x, map.height);
    }
    for (let y = 1; y < map.height; y++) {
      ctx.moveTo(0, y);
      ctx.lineTo(map.width, y);
    }
    ctx.strokeStyle = pal.grid;
    ctx.lineWidth = px;
    ctx.stroke();
  }
  // Spawns: side 0 (attackers) belongs to `frame.attackers` this round.
  map.spawns.forEach((tiles, side) => {
    const team = side === 0 ? frame.attackers : 1 - frame.attackers;
    ctx.fillStyle = alpha(pal.team[team], 0.1);
    ctx.strokeStyle = alpha(pal.team[team], 0.45);
    ctx.lineWidth = px;
    ctx.setLineDash([4 * px, 4 * px]);
    for (const [tx, ty] of tiles) {
      ctx.fillRect(tx, ty, 1, 1);
      ctx.strokeRect(tx + 0.08, ty + 0.08, 0.84, 0.84);
    }
    ctx.setLineDash([]);
  });
  for (const site of map.sites) {
    ctx.fillStyle = alpha(pal.site, 0.16);
    for (const [tx, ty] of site.tiles) ctx.fillRect(tx, ty, 1, 1);
    ctx.strokeStyle = alpha(pal.site, 0.7);
    ctx.lineWidth = 1.5 * px;
    ctx.strokeRect(site.minTx, site.minTy, site.maxTx - site.minTx, site.maxTy - site.minTy);
    pen.inPixels(() => {
      const [sx, sy] = pen.screen((site.minTx + site.maxTx) / 2, (site.minTy + site.maxTy) / 2);
      const size = Math.max(11, Math.min(site.maxTx - site.minTx, site.maxTy - site.minTy) * camera.ppt * 0.55);
      ctx.font = `700 ${size}px ${FONT}`;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillStyle = alpha(pal.site, 0.5);
      ctx.fillText(siteName(site.id), sx, sy);
    });
  }
}

function drawWalls(pen: Pen): void {
  const { ctx, px } = pen;
  const { map, palette: pal } = pen.scene;
  const paths = pathsFor(map);
  ctx.fillStyle = pal.low;
  ctx.fill(paths.low);
  ctx.strokeStyle = pal.lowEdge;
  ctx.lineWidth = 1.2 * px;
  ctx.stroke(paths.low);
  ctx.stroke(paths.lowMarks);
  ctx.fillStyle = pal.wall;
  ctx.fill(paths.walls);
  ctx.strokeStyle = pal.wallEdge;
  ctx.lineWidth = px;
  ctx.strokeRect(0, 0, map.width, map.height);
}

interface Cone {
  team: number;
  points: Vec2[];
}

/** Vision outlines (in tiles) of the units that count: the viewers in a fogged view, otherwise everyone alive. */
function visionCones(scene: Scene, poses: Pose[]): Cone[] {
  const { frame, view, map, rules } = scene;
  const fogged = view.viewerIds.length > 0;
  if (!fogged && !scene.show.vision) return [];
  const out: Cone[] = [];
  frame.units.forEach((u, i) => {
    if (!u.alive || (fogged && !view.viewerIds.includes(u.id))) return;
    const blind = (u.state & S_BLINDED) !== 0;
    const poly = visionPolygon(map, rules, poses[i].x, poses[i].y, poses[i].facing, blind);
    out.push({ team: u.team, points: poly.map(([x, y]) => [x / TILE, y / TILE]) });
  });
  return out;
}

function tracePolygon(ctx: CanvasRenderingContext2D, points: Vec2[]): void {
  ctx.beginPath();
  points.forEach(([x, y], i) => (i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y)));
  ctx.closePath();
}

function drawCones(pen: Pen, cones: Cone[]): void {
  const { ctx, px } = pen;
  for (const c of cones) {
    tracePolygon(ctx, c.points);
    ctx.fillStyle = alpha(pen.scene.palette.team[c.team], 0.07);
    ctx.fill();
    ctx.strokeStyle = alpha(pen.scene.palette.team[c.team], 0.22);
    ctx.lineWidth = px;
    ctx.stroke();
  }
}

/** Darken everything the viewers cannot see: a fog layer with their vision cut out of it. */
function drawFog(pen: Pen, cones: Cone[], width: number, height: number, dpr: number, toTiles: () => void): void {
  const { ctx } = pen;
  const { map, camera: cam, palette: pal } = pen.scene;
  fogCanvas ??= document.createElement("canvas");
  const w = Math.max(1, Math.round(width * dpr));
  const h = Math.max(1, Math.round(height * dpr));
  if (fogCanvas.width !== w || fogCanvas.height !== h) {
    fogCanvas.width = w;
    fogCanvas.height = h;
  }
  const f = fogCanvas.getContext("2d")!;
  f.setTransform(1, 0, 0, 1, 0, 0);
  f.globalCompositeOperation = "source-over";
  f.clearRect(0, 0, w, h);
  f.setTransform(dpr * cam.ppt, 0, 0, dpr * cam.ppt, dpr * (width / 2 - cam.cx * cam.ppt), dpr * (height / 2 - cam.cy * cam.ppt));
  f.fillStyle = pal.fog;
  f.fillRect(0, 0, map.width, map.height);
  f.globalCompositeOperation = "destination-out";
  f.fillStyle = "#000";
  for (const c of cones) {
    tracePolygon(f, c.points);
    f.fill();
  }
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.drawImage(fogCanvas, 0, 0);
  toTiles();
}

/**
 * Where a bullet that is gone in the next frame ended: the first wall on its path, or the unit it damaged
 * (a damage event by its owner close to the path), whichever comes first.
 */
function stopFn(scene: Scene): (p: ProjectileFrame) => Vec2 | null {
  const { map, rules, next } = scene;
  const hits = (next?.events ?? []).filter((e) => e.kind === EVENT_DAMAGE);
  return (p) => {
    const a: Vec2 = [p.x, p.y];
    const b: Vec2 = [p.x + p.vx, p.y + p.vy];
    let best = 1;
    const wall = segmentWall(map, a[0], a[1], b[0], b[1]);
    if (wall) best = closestOnSegment(a, b, wall);
    for (const e of hits) {
      if (e.actor !== p.owner) continue;
      const s = closestOnSegment(a, b, [e.x, e.y]);
      const d = Math.hypot(a[0] + (b[0] - a[0]) * s - e.x, a[1] + (b[1] - a[1]) * s - e.y);
      if (d <= rules.unit_radius * 1.5 && s < best) best = s;
    }
    return best < 1 ? [a[0] + (b[0] - a[0]) * best, a[1] + (b[1] - a[1]) * best] : null;
  };
}

export const FONT = 'ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif';
