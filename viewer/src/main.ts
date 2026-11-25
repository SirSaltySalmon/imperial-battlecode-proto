// Page wiring: loading a replay, the playback clock, input, and keeping canvas and page in step.

import { fitCamera, pan, toTiles, zoomAt, type Camera } from "./camera";
import { fetchEngine, FrameStore } from "./engine";
import { interpUnits } from "./interp";
import { readHeader } from "./ibcr";
import { parseMap, type GameMap } from "./map";
import { ALL, formatPerspective, parsePerspective, viewOf, type Perspective } from "./perspective";
import { render } from "./render";
import { applyTheme, type Palette, type ThemeName } from "./theme";
import { buildTimeline, roundIndexAt, type Timeline } from "./timeline";
import { TILE, type Frame, type Header } from "./types";
import { buildMarkers, Panel, updateBanner, updateFeed } from "./ui";
import { rulesFor, type ViewRules } from "./vision";

/** Ticks per second at 1x. */
const BASE_TPS = 8;
const SPEEDS = [0.25, 0.5, 1, 2, 4, 8, 16];

interface Loaded {
  header: Header;
  store: FrameStore;
  timeline: Timeline;
  map: GameMap;
  rules: ViewRules;
  panel: Panel;
  teamSize: number;
}

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const canvas = $<HTMLCanvasElement>("canvas");
const ctx = canvas.getContext("2d")!;
const stage = $("stage");
const scrub = $<HTMLInputElement>("scrub");
const speedSelect = $<HTMLSelectElement>("speed");
const roundSelect = $<HTMLSelectElement>("round-select");
const playButton = $<HTMLButtonElement>("btn-play");
const fileInput = $<HTMLInputElement>("file");

let loaded: Loaded | null = null;
/** Playback position in frames; the fraction is the way towards the next frame. */
let pos = 0;
let playing = false;
let speed = 1;
let perspective: Perspective = ALL;
const show = { vision: true, sounds: true, debug: true };
let camera: Camera = { cx: 0, cy: 0, ppt: 24 };
/** Until the user pans or zooms, the map is refitted whenever the stage changes size. */
let cameraIsAuto = true;
let themeName: ThemeName = "dark";
let palette: Palette;
let width = 1;
let height = 1;
let dpr = 1;
let raf = 0;
let lastTime = 0;
/** Frame index the page (panel, feed, banner, scrub bar) was last updated for; -1 forces an update. */
let pageFrame = -1;
let loadToken = 0;

function invalidate(page = false): void {
  if (page) pageFrame = -1;
  if (!raf) raf = requestAnimationFrame(onFrame);
}

function onFrame(now: number): void {
  raf = 0;
  if (playing && loaded) {
    const last = loaded.store.count - 1;
    // A background tab can stall for seconds; do not jump the match forward when it comes back.
    // The timestamp of a frame can also lie slightly before the input event that started playback.
    pos += (Math.max(0, Math.min(now - lastTime, 100)) / 1000) * BASE_TPS * speed;
    if (pos >= last) {
      pos = last;
      setPlaying(false);
    }
  }
  lastTime = now;
  draw();
  if (playing) invalidate();
}

function draw(): void {
  if (!loaded) return;
  const { store, map, rules } = loaded;
  const { i, t, frame, next } = moment(loaded);
  const view = viewOf(frame, perspective, map, rules);
  render(ctx, width, height, dpr, {
    map,
    rules,
    frame,
    next,
    t,
    view,
    camera,
    palette,
    show,
    selected: perspective.kind === "unit" ? perspective.id : null,
  });
  if (i !== pageFrame) {
    pageFrame = i;
    loaded.panel.update(frame, perspective, show);
    updateFeed($("feed"), loaded.timeline, i, loaded.teamSize, rules);
    updateBanner($("banner"), loaded.timeline, i, frame, loaded.header, map);
    scrub.value = String(i);
    roundSelect.value = String(roundIndexAt(loaded.timeline, i));
    $("readout").textContent = `tick ${frame.tick} / ${store.count - 1} · ${speed}x`;
  }
}

/** The frame under the playback position, the next one, and how far playback is between them. */
function moment(l: Loaded): { i: number; t: number; frame: Frame; next: Frame | null } {
  const i = Math.max(0, Math.min(Math.floor(pos), l.store.count - 1));
  const next = pos > i && i + 1 < l.store.count ? l.store.get(i + 1) : null;
  return { i, t: next ? pos - i : 0, frame: l.store.get(i), next };
}

function setPlaying(on: boolean): void {
  if (!loaded) return;
  if (on && pos >= loaded.store.count - 1) pos = 0;
  playing = on;
  playButton.textContent = on ? "Pause" : "Play";
  lastTime = performance.now();
  invalidate();
}

function seek(frame: number): void {
  if (!loaded) return;
  pos = Math.max(0, Math.min(loaded.store.count - 1, Math.round(frame)));
  invalidate();
}

/** Step whole ticks from wherever playback is, pausing first. */
function step(delta: number): void {
  setPlaying(false);
  const whole = Math.floor(pos);
  seek(delta < 0 && pos > whole ? whole + delta + 1 : whole + delta);
}

function jumpRound(delta: number): void {
  if (!loaded) return;
  const { rounds } = loaded.timeline;
  const at = roundIndexAt(loaded.timeline, Math.floor(pos));
  // "Previous" first returns to the start of the current round, like a music player.
  const target = delta < 0 && Math.floor(pos) > rounds[at].start ? at : at + delta;
  seek(rounds[Math.max(0, Math.min(rounds.length - 1, target))].start);
}

function setSpeed(value: number): void {
  speed = SPEEDS.includes(value) ? value : 1;
  speedSelect.value = String(speed);
  invalidate(true);
}

function setPerspective(p: Perspective): void {
  // A unit id the match does not have (a stale link) means everyone, not a view that claims to be fogged.
  const known = p.kind !== "unit" || (loaded !== null && p.id < loaded.teamSize * 2);
  perspective = known ? p : ALL;
  invalidate(true);
}

function cycleUnit(delta: number): void {
  if (!loaded) return;
  const n = loaded.teamSize * 2;
  const cur = perspective.kind === "unit" ? perspective.id : delta > 0 ? -1 : n;
  setPerspective({ kind: "unit", id: (cur + delta + n) % n });
}

function toggleLayer(layer: "vision" | "sounds" | "debug"): void {
  show[layer] = !show[layer];
  invalidate(true);
}

function setTheme(name: ThemeName): void {
  themeName = name;
  palette = applyTheme(name);
  $("btn-theme").textContent = name === "dark" ? "Light" : "Dark";
  invalidate();
}

/** The user's own switch is remembered; a theme that came with a link is not. */
function toggleTheme(): void {
  setTheme(themeName === "dark" ? "light" : "dark");
  try {
    localStorage.setItem("ibc-theme", themeName);
  } catch {
    // Storage can be unavailable (private mode); the theme then just does not persist.
  }
}

function fit(): void {
  if (loaded) camera = fitCamera(loaded.map.width, loaded.map.height, width, height);
  cameraIsAuto = true;
  invalidate();
}

function resize(): void {
  const box = stage.getBoundingClientRect();
  width = Math.max(1, box.width);
  height = Math.max(1, box.height);
  dpr = window.devicePixelRatio || 1;
  canvas.width = Math.round(width * dpr);
  canvas.height = Math.round(height * dpr);
  if (cameraIsAuto && loaded) camera = fitCamera(loaded.map.width, loaded.map.height, width, height);
  invalidate();
}

function setStatus(text: string, error = false): void {
  const status = $("status");
  status.textContent = text;
  status.classList.toggle("error", error);
}

let toastTimer = 0;

/** A short-lived message over the map, for errors that leave the open replay in place. */
function showToast(text: string): void {
  const toast = $("toast");
  toast.textContent = text;
  toast.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (toast.hidden = true), 6000);
}

async function openBytes(bytes: Uint8Array, name: string): Promise<void> {
  const token = ++loadToken;
  setPlaying(false);
  setStatus(`Opening ${name}...`);
  let engineTouched = false;
  try {
    const peek = await readHeader(bytes);
    const engine = await fetchEngine(peek.engine_version);
    if (token !== loadToken) return;
    // From here the engine drops the frames of the replay that was open, so that one is gone either way.
    engineTouched = true;
    loaded = null;
    $("empty").hidden = false;
    const { header, frames } = engine.load(bytes);
    const store = new FrameStore(frames);
    const map = parseMap(header.map_text);
    const rules = rulesFor(header.engine_version);
    const first = store.get(0);
    const panel = new Panel($("panel"), header, map, first, rules, { setPerspective, toggle: toggleLayer });
    loaded = { header, store, timeline: buildTimeline(store), map, rules, panel, teamSize: first.units.length / 2 };
  } catch (e) {
    if (token !== loadToken) return;
    const message = `Could not open ${name}:\n${(e as Error).message}`;
    if (!engineTouched && loaded) {
      // The file never reached the engine, so the replay that is open is intact: keep it.
      showToast(message);
      return;
    }
    loaded = null;
    setStatus(message, true);
    $("empty").hidden = false;
    $("panel").replaceChildren();
    $("match-title").textContent = "";
    return;
  }
  setStatus("");
  $("empty").hidden = true;
  $("match-title").textContent = `${loaded.header.bots[0].name} vs ${loaded.header.bots[1].name} · ${loaded.map.name}`;
  scrub.max = String(loaded.store.count - 1);
  buildMarkers($("markers"), loaded.timeline);
  roundSelect.replaceChildren(
    ...loaded.timeline.rounds.map((r, i) => new Option(r.round > 0 ? `Round ${r.round}` : "Draft", String(i))),
  );
  pos = 0;
  perspective = ALL;
  fit();
  invalidate(true);
}

async function openFile(file: File): Promise<void> {
  await openBytes(new Uint8Array(await file.arrayBuffer()), file.name);
}

async function openUrl(url: string): Promise<void> {
  setStatus(`Fetching ${url}...`);
  try {
    const res = await fetch(new URL(url, document.baseURI));
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    await openBytes(new Uint8Array(await res.arrayBuffer()), url.split("/").pop() || url);
  } catch (e) {
    setStatus(`Could not fetch ${url}:\n${(e as Error).message}`, true);
  }
}

/** The unit drawn under a viewport point, if any (only units the perspective shows). */
function unitAt(px: number, py: number): number | null {
  if (!loaded) return null;
  // Hit-test what is on screen: the same in-between poses the canvas draws.
  const { frame, next, t } = moment(loaded);
  const poses = interpUnits(frame.units, next?.units ?? null, t);
  const visible = viewOf(frame, perspective, loaded.map, loaded.rules).unitVisible;
  const [tx, ty] = toTiles(camera, px, py, width, height);
  const reach = Math.max((loaded.rules.unit_radius / TILE) * 1.5, 10 / camera.ppt);
  let best: number | null = null;
  let bestD = reach;
  frame.units.forEach((u, i) => {
    const d = Math.hypot(poses[i].x / TILE - tx, poses[i].y / TILE - ty);
    if (u.alive && visible[i] && d <= bestD) {
      best = u.id;
      bestD = d;
    }
  });
  return best;
}

function bindPointer(): void {
  const pointers = new Map<number, { x: number; y: number }>();
  let moved = 0;
  const local = (e: PointerEvent | WheelEvent | MouseEvent) => {
    const box = canvas.getBoundingClientRect();
    return { x: e.clientX - box.left, y: e.clientY - box.top };
  };
  canvas.addEventListener("pointerdown", (e) => {
    canvas.setPointerCapture(e.pointerId);
    pointers.set(e.pointerId, local(e));
    if (pointers.size === 1) moved = 0;
    canvas.classList.add("panning");
  });
  canvas.addEventListener("pointermove", (e) => {
    const before = pointers.get(e.pointerId);
    if (!before) return;
    const now = local(e);
    if (pointers.size === 1) {
      moved += Math.abs(now.x - before.x) + Math.abs(now.y - before.y);
      if (moved > 4) {
        camera = pan(camera, now.x - before.x, now.y - before.y);
        cameraIsAuto = false;
      }
    } else if (pointers.size === 2) {
      // Pinch: zoom about the midpoint by the change in finger distance.
      const other = [...pointers.entries()].find(([id]) => id !== e.pointerId)![1];
      const d0 = Math.hypot(before.x - other.x, before.y - other.y);
      const d1 = Math.hypot(now.x - other.x, now.y - other.y);
      if (d0 > 0) camera = zoomAt(camera, d1 / d0, (now.x + other.x) / 2, (now.y + other.y) / 2, width, height);
      cameraIsAuto = false;
      moved = 99;
    }
    pointers.set(e.pointerId, now);
    invalidate();
  });
  const release = (e: PointerEvent) => {
    if (!pointers.delete(e.pointerId)) return;
    if (pointers.size === 0) canvas.classList.remove("panning");
    if (e.type === "pointerup" && moved <= 4) {
      const { x, y } = local(e);
      const id = unitAt(x, y);
      if (id !== null) setPerspective({ kind: "unit", id });
    }
  };
  canvas.addEventListener("pointerup", release);
  canvas.addEventListener("pointercancel", release);
  canvas.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      const { x, y } = local(e);
      camera = zoomAt(camera, Math.exp(-e.deltaY * 0.0015), x, y, width, height);
      cameraIsAuto = false;
      invalidate();
    },
    { passive: false },
  );
  canvas.addEventListener("dblclick", (e) => {
    const { x, y } = local(e);
    if (unitAt(x, y) === null) fit();
  });
}

function bindKeys(): void {
  window.addEventListener("keydown", (e) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    // A focused control keeps the keys it uses itself: Space / Enter press a button, arrows move the scrub
    // bar or a select. Everything else stays a viewer shortcut.
    const tag = (e.target as HTMLElement).tagName;
    const own: Record<string, string[]> = {
      BUTTON: [" ", "Enter"],
      SELECT: [" ", "Enter", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End"],
      INPUT: ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End"],
    };
    if (own[tag]?.includes(e.key)) return;
    const speedAt = SPEEDS.indexOf(speed);
    const actions: Record<string, () => void> = {
      " ": () => setPlaying(!playing),
      k: () => setPlaying(!playing),
      ArrowLeft: () => step(e.shiftKey ? -10 : -1),
      ArrowRight: () => step(e.shiftKey ? 10 : 1),
      ArrowUp: () => setSpeed(SPEEDS[Math.min(SPEEDS.length - 1, speedAt + 1)]),
      ArrowDown: () => setSpeed(SPEEDS[Math.max(0, speedAt - 1)]),
      n: () => jumpRound(1),
      p: () => jumpRound(-1),
      Home: () => seek(0),
      End: () => seek(Infinity),
      "1": () => setPerspective(ALL),
      "2": () => setPerspective({ kind: "team", team: 0 }),
      "3": () => setPerspective({ kind: "team", team: 1 }),
      Escape: () => setPerspective(ALL),
      ".": () => cycleUnit(1),
      ",": () => cycleUnit(-1),
      v: () => toggleLayer("vision"),
      s: () => toggleLayer("sounds"),
      d: () => toggleLayer("debug"),
      f: fit,
      t: toggleTheme,
      o: () => fileInput.click(),
      "?": () => ($("help").hidden = !$("help").hidden),
    };
    const action = actions[e.key.length === 1 ? e.key.toLowerCase() : e.key];
    if (!action) return;
    e.preventDefault();
    action();
  });
}

function bindControls(): void {
  // A button clicked with the mouse gives focus back, so Space keeps meaning play / pause afterwards.
  // Keyboard activation (detail 0) keeps focus where the user put it.
  document.addEventListener("click", (e) => {
    const target = e.target instanceof Element ? e.target.closest("button") : null;
    if (target && e.detail > 0) target.blur();
  });
  for (const s of SPEEDS) speedSelect.append(new Option(`${s}x`, String(s)));
  speedSelect.value = "1";
  speedSelect.addEventListener("change", () => setSpeed(Number(speedSelect.value)));
  playButton.addEventListener("click", () => setPlaying(!playing));
  $("btn-step").addEventListener("click", () => step(1));
  $("btn-step-back").addEventListener("click", () => step(-1));
  $("btn-next-round").addEventListener("click", () => jumpRound(1));
  $("btn-prev-round").addEventListener("click", () => jumpRound(-1));
  scrub.addEventListener("input", () => {
    setPlaying(false);
    seek(Number(scrub.value));
  });
  roundSelect.addEventListener("change", () => {
    if (loaded) seek(loaded.timeline.rounds[Number(roundSelect.value)].start);
  });
  $("btn-theme").addEventListener("click", toggleTheme);
  $("btn-help").addEventListener("click", () => ($("help").hidden = !$("help").hidden));
  $("btn-open").addEventListener("click", () => fileInput.click());
  $("btn-open-2").addEventListener("click", () => fileInput.click());
  fileInput.addEventListener("change", () => {
    const file = fileInput.files?.[0];
    if (file) void openFile(file);
    fileInput.value = "";
  });
  window.addEventListener("dragover", (e) => {
    e.preventDefault();
    stage.classList.add("dragover");
  });
  window.addEventListener("dragleave", () => stage.classList.remove("dragover"));
  window.addEventListener("drop", (e) => {
    e.preventDefault();
    stage.classList.remove("dragover");
    const file = e.dataTransfer?.files[0];
    if (file) void openFile(file);
  });
}

function storedTheme(): string | null {
  try {
    return localStorage.getItem("ibc-theme");
  } catch {
    return null;
  }
}

/**
 * Page parameters: `replay=<url>`, and optionally `tick=<n>`, `view=all|a|b|u<id>`, `speed=<x>`,
 * `theme=dark|light`, `play=1`, so a link can point at a moment of a match.
 */
async function start(): Promise<void> {
  const params = new URLSearchParams(location.search);
  const theme = params.get("theme") ?? storedTheme();
  setTheme(theme === "light" ? "light" : "dark");
  bindControls();
  bindPointer();
  bindKeys();
  new ResizeObserver(resize).observe(stage);
  resize();
  const replay = params.get("replay");
  if (!replay) return;
  await openUrl(replay);
  if (!loaded) return;
  if (params.has("speed")) setSpeed(Number(params.get("speed")));
  if (params.has("view")) setPerspective(parsePerspective(params.get("view")));
  if (params.has("tick")) seek(Number(params.get("tick")) || 0);
  if (params.get("play") === "1") setPlaying(true);
}

// A handle for scripted checks (end-to-end tests drive the viewer through it).
Object.assign(window, {
  ibcViewer: {
    seek,
    setPlaying,
    setSpeed,
    setPerspective: (text: string) => setPerspective(parsePerspective(text)),
    /** Jump to a fractional frame position, to look at an interpolated moment while paused. */
    setPosition(p: number) {
      if (!loaded) return;
      pos = Math.max(0, Math.min(loaded.store.count - 1, p));
      invalidate();
    },
    state: () => ({
      loaded: !!loaded,
      frames: loaded?.store.count ?? 0,
      pos,
      playing,
      speed,
      perspective: formatPerspective(perspective),
      rounds: loaded?.timeline.rounds ?? [],
      cachedFrames: loaded?.store.cached ?? 0,
    }),
  },
});

void start();
