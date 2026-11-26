// Whole-map edits and the symmetry helper. `applyOp` and `mirrorHalf` are ports of `ibc-maptool transform`
// and `ibc-maptool mirror` (crates/maptool/src/transform.rs) and give the same grids; `partners` is the
// live version of the same mirrors for painting.

import type { MapDoc } from "./doc";

export type Op = "flip-h" | "flip-v" | "transpose" | "rot90" | "rot180" | "rot270" | "swap-sides";
/** `h`: left half onto the right; `v`: top half onto the bottom; `point`: top half rotated 180 degrees. */
export type Axis = "h" | "v" | "point";
export type Symmetry = "none" | Axis;

function swapSide(c: string): string {
  return c === "a" ? "d" : c === "d" ? "a" : c;
}

function transposeRows(rows: string[]): string[] {
  const w = rows[0]?.length ?? 0;
  const out: string[] = [];
  for (let x = 0; x < w; x++) out.push(rows.map((r) => r[x]).join(""));
  return out;
}

const reverse = (s: string) => [...s].reverse().join("");

/** One transform. Rotations are clockwise on screen. */
export function applyOp(doc: MapDoc, op: Op): MapDoc {
  let rows = [...doc.rows];
  switch (op) {
    case "flip-h":
      rows = rows.map(reverse);
      break;
    case "flip-v":
      rows.reverse();
      break;
    case "transpose":
      rows = transposeRows(rows);
      break;
    case "rot90":
      rows = transposeRows(rows.reverse());
      break;
    case "rot180":
      rows = rows.reverse().map(reverse);
      break;
    case "rot270":
      rows = transposeRows(rows).reverse();
      break;
    case "swap-sides":
      rows = rows.map((r) => [...r].map(swapSide).join(""));
      break;
  }
  return { header: [...doc.header], rows };
}

/** The tile that (x, y) mirrors onto. */
export function mirrored(axis: Axis, w: number, h: number, x: number, y: number): [number, number] {
  if (axis === "h") return [w - 1 - x, y];
  if (axis === "v") return [x, h - 1 - y];
  return [w - 1 - x, h - 1 - y];
}

function inSource(axis: Axis, w: number, h: number, x: number, y: number): boolean {
  return axis === "h" ? x < Math.floor(w / 2) : y < Math.floor(h / 2);
}

/**
 * Overwrite one half with the mirror image of the other. Site digits in the copy are shifted past the kept
 * half's highest (a mirrored site 1 becomes a new site); with `swapSides` spawns trade sides in the copy. A
 * middle row or column is left alone.
 */
export function mirrorHalf(doc: MapDoc, axis: Axis, swapSides: boolean): MapDoc {
  const w = doc.rows[0]?.length ?? 0;
  const h = doc.rows.length;
  const copied = (x: number, y: number) => {
    const [sx, sy] = mirrored(axis, w, h, x, y);
    return !inSource(axis, w, h, x, y) && inSource(axis, w, h, sx, sy);
  };
  let highest = 0;
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const c = doc.rows[y][x];
      if (!copied(x, y) && c >= "0" && c <= "9") highest = Math.max(highest, Number(c));
    }
  }
  const rows = doc.rows.map((row, y) =>
    [...row]
      .map((keep, x) => {
        if (!copied(x, y)) return keep;
        const [sx, sy] = mirrored(axis, w, h, x, y);
        let c = doc.rows[sy][sx];
        if (c >= "0" && c <= "9") {
          const shifted = Number(c) + highest;
          if (shifted > 9) throw new Error(`mirroring would create site ${shifted}; the format allows 1..9`);
          c = String(shifted);
        }
        return swapSides ? swapSide(c) : c;
      })
      .join(""),
  );
  return { header: [...doc.header], rows };
}

export interface SymmetryOptions {
  symmetry: Symmetry;
  /** Spawns change sides in the mirror image (`a` <-> `d`). */
  swapSides: boolean;
  /** Sites pair up in the mirror image: 1 <-> 2, 3 <-> 4, ... (otherwise a site mirrors onto itself). */
  pairSites: boolean;
}

/** The tile a painted tile becomes in the mirror image. */
export function mirrorTile(c: string, opts: SymmetryOptions): string {
  if (opts.swapSides && (c === "a" || c === "d")) return swapSide(c);
  if (opts.pairSites && c >= "1" && c <= "9") {
    const n = Number(c);
    const partner = n % 2 === 1 ? n + 1 : n - 1;
    return partner <= 9 ? String(partner) : c;
  }
  return c;
}

/**
 * Every tile a stroke at (x, y) with tile `c` paints under the symmetry: the tile itself, then its mirror
 * image (unless it is its own mirror).
 */
export function partners(
  w: number,
  h: number,
  x: number,
  y: number,
  c: string,
  opts: SymmetryOptions,
): [number, number, string][] {
  const out: [number, number, string][] = [[x, y, c]];
  if (opts.symmetry === "none") return out;
  const [mx, my] = mirrored(opts.symmetry, w, h, x, y);
  if (mx !== x || my !== y) out.push([mx, my, mirrorTile(c, opts)]);
  return out;
}
