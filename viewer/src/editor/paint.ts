// Paint operations on a `MapDoc`, in place: a square brush, a filled rectangle and a flood fill, each
// repeated on the mirror image when a symmetry is on. The page wraps a whole stroke in one undo step.

import { docHeight, docWidth, setTile, tileOf, type MapDoc } from "./doc";
import { partners, type SymmetryOptions } from "./transform";

export type Tool = "brush" | "rect" | "fill";

/** Paint `c` at one tile and its mirror image. Returns the number of tiles changed. */
export function paintTile(doc: MapDoc, x: number, y: number, c: string, sym: SymmetryOptions): number {
  let changed = 0;
  for (const [px, py, pc] of partners(docWidth(doc), docHeight(doc), x, y, c, sym)) {
    if (setTile(doc, px, py, pc)) changed++;
  }
  return changed;
}

/** A `size` x `size` brush centred on (x, y) (an even size leans up and left). */
export function paintBrush(doc: MapDoc, x: number, y: number, size: number, c: string, sym: SymmetryOptions): number {
  const lo = -Math.floor((size - 1) / 2);
  let changed = 0;
  for (let dy = lo; dy < lo + size; dy++) {
    for (let dx = lo; dx < lo + size; dx++) changed += paintTile(doc, x + dx, y + dy, c, sym);
  }
  return changed;
}

/** Every tile on the straight line between two tiles (Bresenham), so a fast drag leaves no gaps. */
export function lineTiles(x0: number, y0: number, x1: number, y1: number): [number, number][] {
  const out: [number, number][] = [];
  const dx = Math.abs(x1 - x0);
  const dy = -Math.abs(y1 - y0);
  const sx = x0 < x1 ? 1 : -1;
  const sy = y0 < y1 ? 1 : -1;
  let err = dx + dy;
  let [x, y] = [x0, y0];
  for (;;) {
    out.push([x, y]);
    if (x === x1 && y === y1) return out;
    const e2 = 2 * err;
    if (e2 >= dy) {
      err += dy;
      x += sx;
    }
    if (e2 <= dx) {
      err += dx;
      y += sy;
    }
  }
}

/** Fill the rectangle with corners (x0, y0) and (x1, y1), inclusive. */
export function paintRect(doc: MapDoc, x0: number, y0: number, x1: number, y1: number, c: string, sym: SymmetryOptions): number {
  let changed = 0;
  for (let y = Math.min(y0, y1); y <= Math.max(y0, y1); y++) {
    for (let x = Math.min(x0, x1); x <= Math.max(x0, x1); x++) changed += paintTile(doc, x, y, c, sym);
  }
  return changed;
}

/** Replace the 4-connected region of the tile at (x, y) with `c`. */
export function floodFill(doc: MapDoc, x: number, y: number, c: string, sym: SymmetryOptions): number {
  const w = docWidth(doc);
  const h = docHeight(doc);
  if (x < 0 || y < 0 || x >= w || y >= h) return 0;
  const from = tileOf(doc, x, y);
  if (from === c) return 0;
  const seen = new Uint8Array(w * h);
  const region: [number, number][] = [];
  const stack: [number, number][] = [[x, y]];
  while (stack.length) {
    const [cx, cy] = stack.pop()!;
    if (cx < 0 || cy < 0 || cx >= w || cy >= h || seen[cy * w + cx] || tileOf(doc, cx, cy) !== from) continue;
    seen[cy * w + cx] = 1;
    region.push([cx, cy]);
    stack.push([cx + 1, cy], [cx - 1, cy], [cx, cy + 1], [cx, cy - 1]);
  }
  // The region is found first, then painted, so the mirror image of an early tile cannot cut the region.
  let changed = 0;
  for (const [rx, ry] of region) changed += paintTile(doc, rx, ry, c, sym);
  return changed;
}
