// The static map, parsed from the replay header's `map_text` (same grammar as `Map::parse` in the engine),
// and the wall ray test the viewer uses for vision cones and bullet trails. Rendering only: floats are fine.

import { TILE, TILE_FLOOR, TILE_LOW, TILE_WALL, type Vec2 } from "./types";

export interface Site {
  id: number;
  /** Bounding box in tiles, max exclusive. */
  minTx: number;
  minTy: number;
  maxTx: number;
  maxTy: number;
  /** Tiles that belong to the site. */
  tiles: Vec2[];
}

export interface GameMap {
  name: string;
  teamSize: number;
  width: number;
  height: number;
  /** Row-major `TILE_*` values. */
  tiles: Uint8Array;
  sites: Site[];
  /** Spawn tiles: `[attackers, defenders]`. */
  spawns: [Vec2[], Vec2[]];
}

export function parseMap(text: string): GameMap {
  const lines = text.split(/\r?\n/);
  let name = "unnamed";
  let teamSize = 0;
  let i = 0;
  for (; i < lines.length; i++) {
    const line = lines[i].trimEnd();
    if (line === "---") {
      i++;
      break;
    }
    if (line === "" || line.startsWith("//")) continue;
    const colon = line.indexOf(":");
    if (colon < 0) throw new Error(`bad map header line: ${line}`);
    const key = line.slice(0, colon).trim();
    const value = line.slice(colon + 1).trim();
    if (key === "name") name = value;
    if (key === "team_size") teamSize = Number.parseInt(value, 10) || 0;
  }
  const rows = lines
    .slice(i)
    .map((l) => l.trimEnd())
    .filter((l) => l !== "");
  if (rows.length === 0) throw new Error("map has no grid (missing `---`?)");
  const width = rows[0].length;
  const height = rows.length;
  const tiles = new Uint8Array(width * height).fill(TILE_FLOOR);
  const spawns: [Vec2[], Vec2[]] = [[], []];
  const byId = new Map<number, Site>();
  rows.forEach((row, ty) => {
    if (row.length !== width) throw new Error(`map row ${ty} has width ${row.length} but row 0 has ${width}`);
    for (let tx = 0; tx < width; tx++) {
      const ch = row[tx];
      if (ch === "#") tiles[ty * width + tx] = TILE_WALL;
      else if (ch === "o") tiles[ty * width + tx] = TILE_LOW;
      else if (ch === "a") spawns[0].push([tx, ty]);
      else if (ch === "d") spawns[1].push([tx, ty]);
      else if (ch >= "1" && ch <= "9") {
        const id = ch.charCodeAt(0) - 49;
        let s = byId.get(id);
        if (!s) {
          s = { id, minTx: tx, minTy: ty, maxTx: tx + 1, maxTy: ty + 1, tiles: [] };
          byId.set(id, s);
        }
        s.minTx = Math.min(s.minTx, tx);
        s.minTy = Math.min(s.minTy, ty);
        s.maxTx = Math.max(s.maxTx, tx + 1);
        s.maxTy = Math.max(s.maxTy, ty + 1);
        s.tiles.push([tx, ty]);
      } else if (ch !== ".") {
        throw new Error(`unknown map tile \`${ch}\` at ${tx},${ty}`);
      }
    }
  });
  const sites = [...byId.values()].sort((a, b) => a.id - b.id);
  return { name, teamSize, width, height, tiles, sites, spawns };
}

/** Tile at tile coordinates; everything outside the map is wall, as in the engine. */
export function tileAt(map: GameMap, tx: number, ty: number): number {
  if (tx < 0 || ty < 0 || tx >= map.width || ty >= map.height) return TILE_WALL;
  return map.tiles[ty * map.width + tx];
}

/** "A", "B", ... for site ids 0, 1, ... */
export function siteName(id: number): string {
  return String.fromCharCode(65 + id);
}

/**
 * First wall on the segment a -> b (fixed-point world units), or null if the way is clear.
 * Grid traversal over tile boundaries (Amanatides and Woo), the float twin of `Map::segment_wall`.
 */
export function segmentWall(map: GameMap, ax: number, ay: number, bx: number, by: number): Vec2 | null {
  let tx = Math.floor(ax / TILE);
  let ty = Math.floor(ay / TILE);
  if (tileAt(map, tx, ty) === TILE_WALL) return [ax, ay];
  const ex = Math.floor(bx / TILE);
  const ey = Math.floor(by / TILE);
  const dx = bx - ax;
  const dy = by - ay;
  const sx = Math.sign(dx);
  const sy = Math.sign(dy);
  // Parametric distance (0..1 along the segment) to the next vertical / horizontal tile boundary.
  let tMaxX = dx === 0 ? Infinity : ((sx > 0 ? (tx + 1) * TILE : tx * TILE) - ax) / dx;
  let tMaxY = dy === 0 ? Infinity : ((sy > 0 ? (ty + 1) * TILE : ty * TILE) - ay) / dy;
  const tDeltaX = dx === 0 ? Infinity : TILE / Math.abs(dx);
  const tDeltaY = dy === 0 ? Infinity : TILE / Math.abs(dy);
  for (let guard = 0; (tx !== ex || ty !== ey) && guard < 512; guard++) {
    let t: number;
    if (tMaxX <= tMaxY) {
      t = tMaxX;
      tx += sx;
      tMaxX += tDeltaX;
    } else {
      t = tMaxY;
      ty += sy;
      tMaxY += tDeltaY;
    }
    if (t > 1) return null;
    if (tileAt(map, tx, ty) === TILE_WALL) return [ax + dx * t, ay + dy * t];
  }
  return null;
}
