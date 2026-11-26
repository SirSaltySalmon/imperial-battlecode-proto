// Map editor page (`editor.html`). Holds the map being edited and wires the controls; painting is
// `paint.ts`, mirrors and rotations `transform.ts`, validation `check.ts`, the server `api.ts`, pointer and
// keys `input.ts`. Every edit replaces `doc` and goes through `edit`, so undo, the check and the redraw
// cannot be skipped.

import { fitCamera, type Camera } from "../camera";
import { applyTheme, type Palette, type ThemeName } from "../theme";
import { EditorApi, validMapName, type LintResult, type SaveResult } from "./api";
import { check, loadEngineParse, type CheckResult, type EngineParse } from "./check";
import {
  cloneDoc,
  docHeight,
  docWidth,
  getHeader,
  parseDoc,
  resizeDoc,
  scaffold,
  serializeDoc,
  setHeader,
  type MapDoc,
} from "./doc";
import { drawEditor } from "./draw";
import { History } from "./history";
import { bindInput, type InputHost } from "./input";
import { renderCheck, renderLint } from "./panel";
import type { Tool } from "./paint";
import { applyOp, mirrorHalf, type Axis, type Op, type Symmetry, type SymmetryOptions } from "./transform";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const canvas = $<HTMLCanvasElement>("canvas");
const ctx = canvas.getContext("2d")!;
const stage = $("stage");

const TILES: [string, string][] = [
  ["#", "Wall"],
  [".", "Floor"],
  ["o", "Low cover"],
  ["a", "Attacker spawn"],
  ["d", "Defender spawn"],
  ...[1, 2, 3, 4, 5, 6, 7, 8, 9].map((n): [string, string] => [String(n), `Site ${String.fromCharCode(64 + n)}`]),
];

let doc: MapDoc = scaffold(32, 24, 5, "custom1");
const history = new History();
let camera: Camera = { cx: 16, cy: 12, ppt: 20 };
let cameraIsAuto = true;
const size = { width: 1, height: 1, dpr: 1 };
let palette: Palette = applyTheme("dark");
let themeName: ThemeName = "dark";
let tile = "#";
let tool: Tool = "brush";
let brushSize = 1;
const sym: SymmetryOptions = { symmetry: "none", swapSides: false, pairSites: true };
let cursor: [number, number][] = [];
let engine: EngineParse | null = null;
let result: CheckResult | null = null;
let api: EditorApi | null = null;
let lint: LintResult | null = null;
let lintedText = "";
/** The file the map came from (a folder map, an opened file, the last save), for lint's `name-mismatch`. */
let fileStem: string | null = null;
let savedText = serializeDoc(doc);
let raf = 0;
let toastTimer = 0;

function invalidate(): void {
  if (!raf) raf = requestAnimationFrame(draw);
}

function draw(): void {
  raf = 0;
  drawEditor(ctx, size, doc, camera, palette, { cursor, symmetry: sym.symmetry, summary: result?.summary ?? null });
}

function toast(text: string, error = true): void {
  const t = $("toast");
  t.textContent = text;
  t.style.borderColor = error ? "" : "var(--team-a)";
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (t.hidden = true), 4000);
}

/** Replace the doc with `next` as one undo step from `before`, then recheck and redraw. */
function edit(next: MapDoc, before: MapDoc = doc): void {
  history.commit(before, next);
  doc = next;
  refresh();
}

/** Recheck, update the controls from the doc, redraw. */
function refresh(): void {
  const text = serializeDoc(doc);
  if (engine) {
    result = check(doc, engine, text);
    renderCheck($("verdict"), $("summary"), $("findings"), result);
  }
  renderLint($("lint-box"), $("lint-state"), $("lint"), lint, text !== lintedText);
  const name = $<HTMLInputElement>("map-name");
  if (document.activeElement !== name) name.value = getHeader(doc, "name") ?? "";
  name.classList.toggle("invalid", !validMapName(name.value));
  $<HTMLSelectElement>("team-size").value = getHeader(doc, "team_size") ?? "";
  $<HTMLInputElement>("size-w").value = String(docWidth(doc));
  $<HTMLInputElement>("size-h").value = String(docHeight(doc));
  $<HTMLButtonElement>("btn-undo").disabled = !history.canUndo;
  $<HTMLButtonElement>("btn-redo").disabled = !history.canRedo;
  document.title = `${getHeader(doc, "name") ?? "map"}${text !== savedText ? " *" : ""} - Map editor`;
  invalidate();
}

function fit(): void {
  camera = fitCamera(docWidth(doc), docHeight(doc), size.width, size.height);
  cameraIsAuto = true;
  invalidate();
}

function resize(): void {
  const box = stage.getBoundingClientRect();
  size.width = Math.max(1, box.width);
  size.height = Math.max(1, box.height);
  size.dpr = window.devicePixelRatio || 1;
  canvas.width = Math.round(size.width * size.dpr);
  canvas.height = Math.round(size.height * size.dpr);
  if (cameraIsAuto) fit();
  invalidate();
}

/** File stem of a path or URL (`maps/crossfire.ibcmap` -> `crossfire`), or null if it is not a map file. */
function stemOf(path: string): string | null {
  const base = path.split(/[?#]/)[0].split(/[/\\]/).pop() ?? "";
  return base.endsWith(".ibcmap") ? decodeURIComponent(base.slice(0, -".ibcmap".length)) : null;
}

/**
 * Load map text as a new document (not an undo step: opening a file starts a new history). Returns false if
 * nothing was loaded (unreadable text, or the user kept the unsaved map). `stem` is the file it came from.
 */
function openText(text: string, label: string, stem: string | null): boolean {
  let next: MapDoc;
  try {
    next = parseDoc(text);
  } catch (e) {
    toast(`${label}: ${e instanceof Error ? e.message : e}`);
    return false;
  }
  if (serializeDoc(doc) !== savedText && !confirm("Discard the unsaved changes to this map?")) return false;
  history.clear();
  lint = null;
  doc = next;
  fileStem = stem;
  savedText = serializeDoc(doc);
  fit();
  refresh();
  return true;
}

function openFile(f: File): void {
  void f.text().then((t) => openText(t, f.name, stemOf(f.name)));
}

async function openUrl(url: string): Promise<void> {
  try {
    const res = await fetch(url);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    openText(await res.text(), url, stemOf(new URL(url).pathname));
  } catch (e) {
    toast(`could not open ${url}: ${e instanceof Error ? e.message : e}`);
  }
}

function currentName(): string | null {
  const name = getHeader(doc, "name") ?? "";
  if (validMapName(name)) return name;
  toast("Give the map a name of letters, digits, _ or - first (it is also the file name).");
  $("map-name").focus();
  return null;
}

function exportFile(): void {
  const name = currentName();
  if (!name) return;
  const blob = new Blob([serializeDoc(doc)], { type: "text/plain" });
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = `${name}.ibcmap`;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
  savedText = serializeDoc(doc);
  refresh();
}

async function runLint(): Promise<void> {
  if (!api) return;
  const text = serializeDoc(doc);
  try {
    // Lint compares `name:` with the file the map came from, as `ibc-maptool lint <file>` would.
    lint = await api.lint(text, fileStem ?? "");
    lintedText = text;
  } catch (e) {
    toast(`lint: ${e instanceof Error ? e.message : e}`);
  }
  refresh();
}

async function save(): Promise<void> {
  const name = currentName();
  if (!api || !name) return;
  const text = serializeDoc(doc);
  let res: SaveResult;
  try {
    res = await api.save(text, name, false);
    if (!res.ok && res.exists) {
      if (!confirm(`${name}.ibcmap exists in the map folder. Overwrite it?`)) return;
      res = await api.save(text, name, true);
    }
  } catch (e) {
    toast(`save failed, nothing was written: ${e instanceof Error ? e.message : e}`);
    return;
  }
  if (!res.ok) {
    toast(`save: ${res.message}`);
    return;
  }
  savedText = text;
  fileStem = name;
  toast(`saved ${res.file}`, false);
  if (!api.maps.includes(name)) {
    api.maps.push(name);
    api.maps.sort();
    fillServerMaps();
  }
  refresh();
}

function fillServerMaps(): void {
  if (!api) return;
  const select = $<HTMLSelectElement>("server-maps");
  select.replaceChildren(new Option(`Folder (${api.maps.length})`, "", true, true));
  for (const m of api.maps) select.add(new Option(m, m));
  select.hidden = false;
}

function setTheme(name: ThemeName): void {
  themeName = name;
  palette = applyTheme(name);
  $("btn-theme").textContent = name === "dark" ? "Light" : "Dark";
  try {
    localStorage.setItem("ibc-theme", name);
  } catch {
    // Storage can be blocked; the theme then lasts for this page only.
  }
  invalidate();
}

function guarded(f: () => MapDoc): void {
  try {
    edit(f());
  } catch (e) {
    toast(e instanceof Error ? e.message : String(e));
  }
}

function newMap(): void {
  const dialog = $<HTMLDialogElement>("new-dialog");
  const form = dialog.querySelector("form")!;
  dialog.onclose = () => {
    if (dialog.returnValue !== "ok") return;
    const f = new FormData(form);
    const name = String(f.get("name") ?? "");
    try {
      if (!validMapName(name)) throw new Error("a name is letters, digits, _ or -");
      const fresh = scaffold(Number(f.get("w")), Number(f.get("h")), Number(f.get("team")), name);
      if (openText(serializeDoc(fresh), "new map", null)) {
        savedText = "";
        refresh();
      }
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e));
    }
  };
  dialog.showModal();
}

function setTile(c: string): void {
  tile = c;
  document.querySelectorAll<HTMLButtonElement>("#palette button").forEach((b) => b.classList.toggle("on", b.dataset.tile === c));
}

function setTool(t: Tool): void {
  tool = t;
  document.querySelectorAll<HTMLButtonElement>("#tool-seg button").forEach((b) => b.classList.toggle("on", b.dataset.tool === t));
}

function undo(): void {
  const prev = history.undo(doc);
  if (prev) {
    doc = prev;
    refresh();
  }
}

function redo(): void {
  const next = history.redo(doc);
  if (next) {
    doc = next;
    refresh();
  }
}

function bindHeader(): void {
  const name = $<HTMLInputElement>("map-name");
  name.addEventListener("change", () => {
    const next = cloneDoc(doc);
    setHeader(next, "name", name.value.trim());
    edit(next);
  });
  const team = $<HTMLSelectElement>("team-size");
  team.add(new Option("auto", ""));
  for (let n = 1; n <= 5; n++) team.add(new Option(String(n), String(n)));
  team.addEventListener("change", () => {
    const next = cloneDoc(doc);
    if (team.value) setHeader(next, "team_size", team.value);
    else next.header = next.header.filter((l) => !/^team_size\s*:/.test(l));
    edit(next);
  });
  const file = $<HTMLInputElement>("file");
  $("btn-new").addEventListener("click", newMap);
  $("btn-open").addEventListener("click", () => file.click());
  file.addEventListener("change", () => {
    const f = file.files?.[0];
    if (f) openFile(f);
    file.value = "";
  });
  $<HTMLSelectElement>("server-maps").addEventListener("change", (e) => {
    const select = e.target as HTMLSelectElement;
    if (api && select.value) void openUrl(api.mapUrl(select.value));
    select.value = "";
  });
  $("btn-undo").addEventListener("click", undo);
  $("btn-redo").addEventListener("click", redo);
  $("btn-export").addEventListener("click", exportFile);
  $("btn-lint").addEventListener("click", () => void runLint());
  $("btn-save").addEventListener("click", () => void save());
  $("btn-theme").addEventListener("click", () => setTheme(themeName === "dark" ? "light" : "dark"));
}

function bindTools(): void {
  const palette = $("palette");
  for (const [c, label] of TILES) {
    const b = document.createElement("button");
    b.dataset.tile = c;
    b.title = `${label} (\`${c}\`)`;
    b.innerHTML = `<code>${c}</code>`;
    b.append(label);
    b.addEventListener("click", () => setTile(c));
    palette.append(b);
  }
  document.querySelectorAll<HTMLButtonElement>("#tool-seg button").forEach((b) =>
    b.addEventListener("click", () => setTool(b.dataset.tool as Tool)),
  );
  const brush = $<HTMLSelectElement>("brush-size");
  for (const n of [1, 2, 3, 5]) brush.add(new Option(`${n} × ${n}`, String(n)));
  brush.addEventListener("change", () => (brushSize = Number(brush.value)));
  $<HTMLSelectElement>("symmetry").addEventListener("change", (e) => {
    sym.symmetry = (e.target as HTMLSelectElement).value as Symmetry;
    invalidate();
  });
  $<HTMLInputElement>("swap-sides").addEventListener("change", (e) => (sym.swapSides = (e.target as HTMLInputElement).checked));
  $<HTMLInputElement>("pair-sites").addEventListener("change", (e) => (sym.pairSites = (e.target as HTMLInputElement).checked));
  document.querySelectorAll<HTMLButtonElement>("[data-op]").forEach((b) =>
    b.addEventListener("click", () => {
      guarded(() => applyOp(doc, b.dataset.op as Op));
      fit();
    }),
  );
  $("btn-mirror").addEventListener("click", () =>
    guarded(() => mirrorHalf(doc, $<HTMLSelectElement>("mirror-axis").value as Axis, sym.swapSides)),
  );
  $("btn-resize").addEventListener("click", () => {
    guarded(() => resizeDoc(doc, Number($<HTMLInputElement>("size-w").value), Number($<HTMLInputElement>("size-h").value)));
    fit();
  });
  setTile("#");
  setTool("brush");
}

const host: InputHost = {
  canvas,
  size,
  get doc() {
    return doc;
  },
  get camera() {
    return camera;
  },
  set camera(c: Camera) {
    camera = c;
    cameraIsAuto = false;
  },
  get tile() {
    return tile;
  },
  get tool() {
    return tool;
  },
  get brushSize() {
    return brushSize;
  },
  sym,
  edit: (next, before) => edit(next, before),
  preview(next) {
    doc = next;
    invalidate();
  },
  setCursor(tiles, readout) {
    cursor = tiles;
    $("readout").textContent = readout;
    invalidate();
  },
  pick: setTile,
  setTool,
  fit,
  undo,
  redo,
  save: () => (api ? void save() : exportFile()),
  lint: () => void runLint(),
  toggleTheme: () => setTheme(themeName === "dark" ? "light" : "dark"),
  open: () => $("file").click(),
  openFile,
};

async function start(): Promise<void> {
  let theme: string | null = null;
  try {
    theme = localStorage.getItem("ibc-theme");
  } catch {
    // No storage: dark.
  }
  setTheme(theme === "light" ? "light" : "dark");
  bindHeader();
  bindTools();
  bindInput(host);
  window.addEventListener("beforeunload", (e) => {
    if (serializeDoc(doc) !== savedText) e.preventDefault();
  });
  new ResizeObserver(resize).observe(stage);
  resize();
  refresh();
  const [engineResult, server] = await Promise.allSettled([loadEngineParse(), EditorApi.detect()]);
  if (engineResult.status === "fulfilled") {
    engine = engineResult.value.parse;
  } else {
    $("verdict").textContent = `No engine check: ${engineResult.reason instanceof Error ? engineResult.reason.message : engineResult.reason}`;
  }
  if (server.status === "fulfilled" && server.value) {
    api = server.value;
    $("btn-lint").hidden = false;
    $("btn-save").hidden = false;
    fillServerMaps();
  }
  const mapParam = new URLSearchParams(location.search).get("map");
  if (mapParam) await openUrl(new URL(mapParam, document.baseURI).href);
  refresh();
}

void start();
