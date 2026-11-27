// Draws the map being edited with the replay viewer's renderer (`render.ts`: floor, spawns, sites, low cover,
// walls) on an empty frame, plus the editor's marks: the symmetry axis, the tiles under the cursor, the spawn
// tiles a team actually uses and the plant points the engine reports.

import type { Camera } from "../camera";
import { parseMap, type GameMap } from "../map";
import { ALL, viewOf } from "../perspective";
import { render, type Pen } from "../render";
import { alpha, type Palette } from "../theme";
import { PHASE_ACTION, TILE, type Frame } from "../types";
import { rulesFor } from "../vision";
import type { MapSummary } from "./check";
import { serializeDoc, type MapDoc } from "./doc";
import type { Symmetry } from "./transform";

const EMPTY_FRAME: Frame = {
  tick: 0,
  round: 1,
  phase: PHASE_ACTION,
  phase_left: 0,
  score: [0, 0],
  attackers: 0,
  charge: { state: 0, x: 0, y: 0 },
  units: [],
  projectiles: [],
  effects: [],
  drops: [],
  events: [],
  debug: [],
  winner: null,
};
const RULES = rulesFor(null);

export interface Overlay {
  /** Tiles the next click paints (brush footprint, rectangle preview), mirror images included. */
  cursor: [number, number][];
  symmetry: Symmetry;
  summary: MapSummary | null;
}

const cache = new WeakMap<MapDoc, GameMap>();

/** The renderer's map for a doc (cached per doc object: the page replaces the doc on every edit). */
export function gameMapOf(doc: MapDoc): GameMap {
  let map = cache.get(doc);
  if (!map) {
    map = parseMap(serializeDoc(doc));
    cache.set(doc, map);
  }
  return map;
}

export function drawEditor(
  ctx: CanvasRenderingContext2D,
  size: { width: number; height: number; dpr: number },
  doc: MapDoc,
  camera: Camera,
  palette: Palette,
  overlay: Overlay,
): void {
  const map = gameMapOf(doc);
  render(ctx, size.width, size.height, size.dpr, {
    map,
    rules: RULES,
    frame: EMPTY_FRAME,
    next: null,
    t: 0,
    view: viewOf(EMPTY_FRAME, ALL, map, RULES),
    camera,
    palette,
    show: { vision: false, debug: false, sounds: false },
    selected: null,
    marks: (pen) => drawMarks(pen, map, overlay),
  });
}

function drawMarks(pen: Pen, map: GameMap, overlay: Overlay): void {
  const { ctx, px } = pen;
  const pal = pen.scene.palette;
  drawAxis(pen, map, overlay.symmetry);
  if (overlay.summary) {
    // The spawn tiles the engine uses: the first `team_size` of each side in reading order.
    map.spawns.forEach((tiles, side) => {
      ctx.fillStyle = pal.team[side];
      for (const [tx, ty] of tiles.slice(0, overlay.summary!.team_size)) {
        ctx.beginPath();
        ctx.arc(tx + 0.5, ty + 0.5, 0.16, 0, Math.PI * 2);
        ctx.fill();
      }
    });
    // Where bots plant: each site's plant point, the centre of the site tile nearest its box centre.
    ctx.strokeStyle = pal.site;
    ctx.lineWidth = 2 * px;
    for (const s of overlay.summary.sites) {
      const [x, y] = [s.plant_x / TILE, s.plant_y / TILE];
      ctx.beginPath();
      ctx.moveTo(x - 0.3, y - 0.3);
      ctx.lineTo(x + 0.3, y + 0.3);
      ctx.moveTo(x + 0.3, y - 0.3);
      ctx.lineTo(x - 0.3, y + 0.3);
      ctx.stroke();
    }
  }
  ctx.strokeStyle = alpha(pal.warn, 0.9);
  ctx.fillStyle = alpha(pal.warn, 0.18);
  ctx.lineWidth = 1.5 * px;
  for (const [tx, ty] of overlay.cursor) {
    ctx.fillRect(tx, ty, 1, 1);
    ctx.strokeRect(tx + 0.04, ty + 0.04, 0.92, 0.92);
  }
}

function drawAxis(pen: Pen, map: GameMap, symmetry: Symmetry): void {
  if (symmetry === "none") return;
  const { ctx, px } = pen;
  ctx.strokeStyle = alpha(pen.scene.palette.warn, 0.55);
  ctx.lineWidth = 1.5 * px;
  ctx.setLineDash([6 * px, 4 * px]);
  ctx.beginPath();
  if (symmetry === "h") {
    ctx.moveTo(map.width / 2, 0);
    ctx.lineTo(map.width / 2, map.height);
  } else if (symmetry === "v") {
    ctx.moveTo(0, map.height / 2);
    ctx.lineTo(map.width, map.height / 2);
  } else {
    const r = 0.6;
    ctx.arc(map.width / 2, map.height / 2, r, 0, Math.PI * 2);
  }
  ctx.stroke();
  ctx.setLineDash([]);
}
