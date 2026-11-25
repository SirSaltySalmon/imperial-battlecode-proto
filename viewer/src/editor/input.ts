// Pointer and keyboard for the editor. A stroke (button down to button up) paints into a copy of the map
// that is shown as it grows and becomes one undo step when the button is released.

import { pan, toTiles, zoomAt, type Camera } from "../camera";
import { cloneDoc, docHeight, docWidth, tileOf, type MapDoc } from "./doc";
import { floodFill, lineTiles, paintBrush, paintRect, type Tool } from "./paint";
import { mirrored, type SymmetryOptions } from "./transform";

export interface InputHost {
  canvas: HTMLCanvasElement;
  size: { width: number; height: number };
  readonly doc: MapDoc;
  camera: Camera;
  readonly tile: string;
  readonly tool: Tool;
  readonly brushSize: number;
  sym: SymmetryOptions;
  /** Commit `next` as one undo step from `before`. */
  edit(next: MapDoc, before: MapDoc): void;
  /** Show `next` during a stroke, without an undo step. */
  preview(next: MapDoc): void;
  setCursor(tiles: [number, number][], readout: string): void;
  pick(tile: string): void;
  setTool(tool: Tool): void;
  fit(): void;
  undo(): void;
  redo(): void;
  save(): void;
  lint(): void;
  toggleTheme(): void;
  open(): void;
  openFile(file: File): void;
}

interface Stroke {
  before: MapDoc;
  work: MapDoc;
  start: [number, number];
  last: [number, number];
}

const KEY_TILES: Record<string, string> = { w: "#", e: ".", c: "o", a: "a", d: "d" };
const KEY_TOOLS: Record<string, Tool> = { b: "brush", r: "rect", g: "fill" };

/** The tiles a click at (x, y) would paint, mirror images included (for the cursor). */
function footprint(host: InputHost, x: number, y: number, stroke: Stroke | null): [number, number][] {
  const w = docWidth(host.doc);
  const h = docHeight(host.doc);
  const out: [number, number][] = [];
  if (host.tool === "rect" && stroke) {
    const [x0, y0] = stroke.start;
    for (let ty = Math.min(y0, y); ty <= Math.max(y0, y); ty++) {
      for (let tx = Math.min(x0, x); tx <= Math.max(x0, x); tx++) out.push([tx, ty]);
    }
  } else {
    const n = host.tool === "brush" ? host.brushSize : 1;
    const lo = -Math.floor((n - 1) / 2);
    for (let dy = lo; dy < lo + n; dy++) for (let dx = lo; dx < lo + n; dx++) out.push([x + dx, y + dy]);
  }
  const inside = out.filter(([tx, ty]) => tx >= 0 && ty >= 0 && tx < w && ty < h);
  if (host.sym.symmetry === "none") return inside;
  const axis = host.sym.symmetry;
  return [...inside, ...inside.map(([tx, ty]) => mirrored(axis, w, h, tx, ty))];
}

function paintStroke(host: InputHost, stroke: Stroke, x: number, y: number): void {
  // A fresh object every step, so the renderer's per-doc cache sees the change.
  const work = host.tool === "rect" ? cloneDoc(stroke.before) : cloneDoc(stroke.work);
  if (host.tool === "brush") {
    for (const [lx, ly] of lineTiles(stroke.last[0], stroke.last[1], x, y)) {
      paintBrush(work, lx, ly, host.brushSize, host.tile, host.sym);
    }
  } else if (host.tool === "rect") {
    paintRect(work, stroke.start[0], stroke.start[1], x, y, host.tile, host.sym);
  }
  stroke.work = work;
  stroke.last = [x, y];
  host.preview(work);
}

export function bindInput(host: InputHost): void {
  const { canvas } = host;
  let stroke: Stroke | null = null;
  let panFrom: { x: number; y: number } | null = null;
  const local = (e: MouseEvent) => {
    const box = canvas.getBoundingClientRect();
    return { x: e.clientX - box.left, y: e.clientY - box.top };
  };
  const tileAt = (e: MouseEvent): [number, number] => {
    const { x, y } = local(e);
    const [tx, ty] = toTiles(host.camera, x, y, host.size.width, host.size.height);
    return [Math.floor(tx), Math.floor(ty)];
  };
  const hover = (e: MouseEvent) => {
    const [x, y] = tileAt(e);
    const inside = x >= 0 && y >= 0 && x < docWidth(host.doc) && y < docHeight(host.doc);
    const readout = inside
      ? `(${x}, ${y})  \`${tileOf(host.doc, x, y)}\`  ·  ${docWidth(host.doc)} × ${docHeight(host.doc)}`
      : `${docWidth(host.doc)} × ${docHeight(host.doc)}`;
    host.setCursor(inside || stroke ? footprint(host, x, y, stroke) : [], readout);
  };

  canvas.addEventListener("contextmenu", (e) => e.preventDefault());
  canvas.addEventListener("pointerdown", (e) => {
    canvas.setPointerCapture(e.pointerId);
    if (e.button !== 0) {
      panFrom = local(e);
      return;
    }
    const [x, y] = tileAt(e);
    if (e.altKey) {
      host.pick(tileOf(host.doc, x, y));
      return;
    }
    if (host.tool === "fill") {
      const work = cloneDoc(host.doc);
      if (floodFill(work, x, y, host.tile, host.sym) > 0) host.edit(work, host.doc);
      return;
    }
    stroke = { before: host.doc, work: host.doc, start: [x, y], last: [x, y] };
    paintStroke(host, stroke, x, y);
    hover(e);
  });
  canvas.addEventListener("pointermove", (e) => {
    if (panFrom) {
      const now = local(e);
      host.camera = pan(host.camera, now.x - panFrom.x, now.y - panFrom.y);
      panFrom = now;
    } else if (stroke) {
      const [x, y] = tileAt(e);
      if (x !== stroke.last[0] || y !== stroke.last[1]) paintStroke(host, stroke, x, y);
    }
    hover(e);
  });
  const release = () => {
    panFrom = null;
    if (!stroke) return;
    const { before, work } = stroke;
    stroke = null;
    host.edit(work, before);
  };
  canvas.addEventListener("pointerup", release);
  canvas.addEventListener("pointercancel", release);
  canvas.addEventListener("pointerleave", () => {
    if (!stroke) host.setCursor([], "");
  });
  canvas.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      const { x, y } = local(e);
      host.camera = zoomAt(host.camera, Math.exp(-e.deltaY * 0.0015), x, y, host.size.width, host.size.height);
      hover(e);
    },
    { passive: false },
  );
  bindKeys(host, () => stroke !== null);
  bindDrop(host);
}

/** `painting`: a stroke is in progress; keyboard edits wait until it is committed. */
function bindKeys(host: InputHost, painting: () => boolean): void {
  window.addEventListener("keydown", (e) => {
    const target = e.target as HTMLElement;
    if (painting() || target.closest("input, select, textarea, dialog")) return;
    const key = e.key.toLowerCase();
    const mod = e.ctrlKey || e.metaKey;
    if (mod && key === "z") host[e.shiftKey ? "redo" : "undo"]();
    else if (mod && key === "y") host.redo();
    else if (mod && key === "s") host.save();
    else if (mod && key === "o") host.open();
    else if (mod || e.altKey) return;
    else if (KEY_TILES[key]) host.pick(KEY_TILES[key]);
    else if (/^[1-9]$/.test(key)) host.pick(key);
    else if (KEY_TOOLS[key]) host.setTool(KEY_TOOLS[key]);
    else if (key === "f") host.fit();
    else if (key === "l") host.lint();
    else if (key === "t") host.toggleTheme();
    else return;
    e.preventDefault();
  });
}

function bindDrop(host: InputHost): void {
  const stage = host.canvas.parentElement!;
  window.addEventListener("dragover", (e) => {
    e.preventDefault();
    stage.classList.add("dragover");
  });
  window.addEventListener("dragleave", () => stage.classList.remove("dragover"));
  window.addEventListener("drop", (e) => {
    e.preventDefault();
    stage.classList.remove("dragover");
    const file = e.dataTransfer?.files[0];
    if (file) host.openFile(file);
  });
}
