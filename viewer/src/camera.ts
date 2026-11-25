// Pan and zoom. The camera is a centre in tile coordinates and a scale in CSS pixels per tile.

export interface Camera {
  cx: number;
  cy: number;
  ppt: number;
}

export const MIN_PPT = 4;
export const MAX_PPT = 160;

/** Centre the whole map in a `width` x `height` viewport with a small margin. */
export function fitCamera(mapW: number, mapH: number, width: number, height: number): Camera {
  const margin = 16;
  const ppt = Math.min((width - margin * 2) / mapW, (height - margin * 2) / mapH);
  return { cx: mapW / 2, cy: mapH / 2, ppt: clamp(ppt, MIN_PPT, MAX_PPT) };
}

/** Zoom by `factor` keeping the tile under the viewport point (px, py) where it is. */
export function zoomAt(c: Camera, factor: number, px: number, py: number, width: number, height: number): Camera {
  const ppt = clamp(c.ppt * factor, MIN_PPT, MAX_PPT);
  const wx = c.cx + (px - width / 2) / c.ppt;
  const wy = c.cy + (py - height / 2) / c.ppt;
  return { cx: wx - (px - width / 2) / ppt, cy: wy - (py - height / 2) / ppt, ppt };
}

export function pan(c: Camera, dxPx: number, dyPx: number): Camera {
  return { ...c, cx: c.cx - dxPx / c.ppt, cy: c.cy - dyPx / c.ppt };
}

/** Viewport pixel -> tile coordinates. */
export function toTiles(c: Camera, px: number, py: number, width: number, height: number): [number, number] {
  return [c.cx + (px - width / 2) / c.ppt, c.cy + (py - height / 2) / c.ppt];
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, v));
}
