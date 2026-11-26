// The play page of `ibc play`: draws the player's team's fogged view with the replay renderer and sends
// keyboard and mouse input to the local server. Real-time mode polls the state on the server's clock; turn
// mode advances one tick per Space.

import { toTiles, type Camera } from "../camera";
import { parseMap, type GameMap } from "../map";
import { viewOf } from "../perspective";
import { render } from "../render";
import { applyTheme, type Palette } from "../theme";
import { S_ALIVE, TILE, type Frame } from "../types";
import { rulesFor, type ViewRules } from "../vision";
import { api, type PlayUnit, type Setup, type State } from "./api";
import { heldInput, heldKey, keyAction, NOTHING_HELD, sameHeld, toWorld, type Action, type Held, type InputMsg } from "./controls";
import { buildMenus, updateHud } from "./hud";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const canvas = $<HTMLCanvasElement>("canvas");
const ctx = canvas.getContext("2d")!;
const stage = $("stage");

let setup: Setup;
let map: GameMap;
let rules: ViewRules;
let palette: Palette;
let state: State | null = null;
let camera: Camera = { cx: 0, cy: 0, ppt: 28 };
let width = 1;
let height = 1;
let dpr = 1;
let held: Held = { ...NOTHING_HELD };
/** Mouse position over the canvas in CSS pixels, or null when it is outside. */
let mouse: { x: number; y: number } | null = null;
/** The held controls last sent, and whether a held update is waiting in the send queue. */
let sentHeld: InputMsg = {};
let heldQueued = false;
/** Every write to the server goes through one queue, so a key-up never overtakes its key-down. */
let chain: Promise<unknown> = Promise.resolve();
let raf = 0;
let stepping = false;
/** Where enemies died, from the kill events (the server only gives the position in the tick of the kill). */
const deaths = new Map<number, [number, number]>();

function invalidate(): void {
  if (!raf) raf = requestAnimationFrame(draw);
}

function enqueue<T>(send: () => Promise<T>): Promise<T> {
  const p = chain.then(send);
  chain = p.catch(() => undefined);
  return p;
}

/** The frame with blanked enemies placed where they died, if that is known, and which units to draw. */
function displayed(frame: Frame): { frame: Frame; hidden: boolean[] } {
  const units = (frame.units as PlayUnit[]).map((u) => {
    const at = !u.alive ? deaths.get(u.id) : undefined;
    return u.hidden && at ? { ...u, x: at[0], y: at[1], hidden: false } : u;
  });
  return { frame: { ...frame, units }, hidden: units.map((u) => u.hidden === true) };
}

function draw(): void {
  raf = 0;
  if (!state) return;
  const { me } = state.view;
  const { frame, hidden } = displayed(state.view.frame);
  // Follow the unit while it is alive; before the first round (and after death) keep the last centre, or
  // the middle of the map.
  if (me.state & S_ALIVE) camera = { ...camera, cx: me.x / TILE, cy: me.y / TILE };
  else if (camera.cx === 0 && camera.cy === 0) camera = { ...camera, cx: map.width / 2, cy: map.height / 2 };
  const view = viewOf(frame, { kind: "team", team: setup.team }, map, rules);
  view.unitVisible = view.unitVisible.map((v, i) => v && !hidden[i]);
  // The server blanks the charge when the team does not know where it is.
  if (frame.charge.state < 0) view.chargeVisible = false;
  render(ctx, width, height, dpr, {
    map,
    rules,
    frame,
    next: null,
    t: 0,
    view,
    camera,
    palette,
    show: { vision: true, debug: false, sounds: true },
    selected: setup.unit,
  });
}

function resize(): void {
  const box = stage.getBoundingClientRect();
  width = Math.max(1, box.width);
  height = Math.max(1, box.height);
  dpr = window.devicePixelRatio || 1;
  canvas.width = Math.round(width * dpr);
  canvas.height = Math.round(height * dpr);
  invalidate();
}

function show(s: State): void {
  state = s;
  for (const u of s.view.frame.units as PlayUnit[]) {
    if (u.alive) deaths.delete(u.id);
    else if (!u.hidden) deaths.set(u.id, [u.x, u.y]);
  }
  updateHud(setup, rules, s);
  $("btn-run").textContent = s.running ? "Pause (Space)" : "Start (Space)";
  // The camera follows the unit, so the world point under a still mouse moves: re-aim.
  pushHeld();
  invalidate();
}

function report(e: unknown): void {
  $("error").textContent = (e as Error).message;
  $("error").hidden = false;
}

/** The world point under the mouse, if the mouse is over the map. */
function aim(): [number, number] | null {
  if (!mouse) return null;
  const [tx, ty] = toTiles(camera, mouse.x, mouse.y, width, height);
  return toWorld(tx, ty);
}

/**
 * Send the held controls if they changed. Updates made while one waits in the queue fold into it: it
 * reads the controls when its turn comes.
 */
function pushHeld(): void {
  if (heldQueued) return;
  heldQueued = true;
  enqueue(async () => {
    heldQueued = false;
    const msg = heldInput(held, aim());
    if (sameHeld(msg, sentHeld)) return;
    sentHeld = msg;
    await api.input(msg);
  }).catch(report);
}

function sendAction(a: Action): void {
  enqueue(() => api.input({ actions: [a] }))
    .then(() => (setup.mode === "turn" ? api.state().then(show) : undefined))
    .catch(report);
}

async function step(): Promise<void> {
  if (stepping) return;
  stepping = true;
  try {
    const s = await enqueue(() => {
      sentHeld = heldInput(held, aim());
      return api.step(sentHeld);
    });
    show(s);
  } catch (e) {
    report(e);
  } finally {
    stepping = false;
  }
}

function toggleRunning(): void {
  if (!state) return;
  const running = !state.running;
  enqueue(() => api.run(running))
    .then(show)
    .catch(report);
}

function bindKeys(): void {
  window.addEventListener("keydown", (e) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const h = heldKey(e.code);
    if (h) {
      e.preventDefault();
      held = { ...held, [h]: true };
      pushHeld();
      return;
    }
    if (e.repeat && e.code !== "Space") return;
    if (e.code === "Space") {
      e.preventDefault();
      if (setup.mode === "turn") void step();
      else if (!e.repeat) toggleRunning();
      return;
    }
    if (e.code === "KeyP" && setup.mode === "realtime") return toggleRunning();
    const a = state ? keyAction(e.code, state.view.match.phase) : null;
    if (a) {
      e.preventDefault();
      sendAction(a);
    }
  });
  window.addEventListener("keyup", (e) => {
    const h = heldKey(e.code);
    if (!h) return;
    held = { ...held, [h]: false };
    pushHeld();
  });
  // A key released while the page had no focus would otherwise stay held.
  window.addEventListener("blur", () => {
    held = { ...NOTHING_HELD };
    pushHeld();
  });
}

function bindMouse(): void {
  const local = (e: MouseEvent) => {
    const box = canvas.getBoundingClientRect();
    return { x: e.clientX - box.left, y: e.clientY - box.top };
  };
  canvas.addEventListener("mousemove", (e) => {
    mouse = local(e);
    pushHeld();
  });
  canvas.addEventListener("mouseleave", () => {
    held = { ...held, fire: false };
    pushHeld();
  });
  canvas.addEventListener("mousedown", (e) => {
    if (e.button !== 0) return;
    mouse = local(e);
    held = { ...held, fire: true };
    pushHeld();
  });
  window.addEventListener("mouseup", (e) => {
    if (e.button !== 0) return;
    held = { ...held, fire: false };
    pushHeld();
  });
  canvas.addEventListener("contextmenu", (e) => e.preventDefault());
  canvas.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      camera = { ...camera, ppt: Math.max(8, Math.min(80, camera.ppt * Math.exp(-e.deltaY * 0.0015))) };
      invalidate();
    },
    { passive: false },
  );
}

/** Real-time mode: fetch the state about twice per tick, until the match is over or the server is gone. */
function poll(): void {
  const period = Math.max(20, 500 / Math.max(1, setup.tps));
  api
    .state()
    .then((s) => {
      show(s);
      if (!s.done) window.setTimeout(poll, period);
    })
    .catch(() => report(new Error("ibc play is not running any more: start it again to play")));
}

function bindButtons(): void {
  // A button clicked with the mouse gives focus back, so Space does not also press it.
  document.addEventListener("click", (e) => {
    const target = e.target instanceof Element ? e.target.closest("button") : null;
    if (target && e.detail > 0) target.blur();
  });
  $("btn-step").addEventListener("click", () => void step());
  $("btn-run").addEventListener("click", toggleRunning);
  $("btn-quit").addEventListener("click", () => {
    api
      .quit()
      .then(() => ($("btn-quit").textContent = "Stopped"))
      .catch(report);
  });
  $("btn-help").addEventListener("click", () => ($("help").hidden = !$("help").hidden));
}

async function start(): Promise<void> {
  palette = applyTheme("dark");
  setup = await api.setup();
  map = parseMap(setup.map_text);
  rules = rulesFor(setup.rules);
  $("match-title").textContent = `${setup.bots[0]} vs ${setup.bots[1]} · ${map.name} · you are unit ${setup.unit} (team ${"AB"[setup.team]}, ${setup.mode})`;
  $("btn-step").hidden = setup.mode !== "turn";
  $("btn-run").hidden = setup.mode !== "realtime";
  buildMenus(setup, sendAction);
  bindKeys();
  bindMouse();
  bindButtons();
  new ResizeObserver(resize).observe(stage);
  resize();
  if (setup.mode === "realtime") poll();
  else show(await api.state());
}

// A handle for scripted checks (end-to-end tests drive the page through it).
Object.assign(window, {
  ibcPlay: {
    state: () => state,
    step,
    send: sendAction,
  },
});

start().catch(report);
