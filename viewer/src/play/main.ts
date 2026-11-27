// The play page of `ibc play`: draws the player's team's fogged view with the replay renderer and sends
// keyboard and mouse input to the local server. The page follows the match with a long poll (each tick
// arrives when it runs) and draws the motion between ticks smoothly. In active mode time passes while the
// player acts; Space waits (a tick per press, more while held), P lets time run, Tab switches unit.

import { toTiles, type Camera } from "../camera";
import { drawBlast, drawHeadline, ENDING_TICKS, endingOf, type Ending } from "../ending";
import { interpUnits } from "../interp";
import { parseMap, type GameMap } from "../map";
import { viewOf } from "../perspective";
import { render } from "../render";
import { applyTheme, type Palette } from "../theme";
import { EVENT_DETONATE, EVENT_ROUND_END, S_ALIVE, TILE, type EventFrame, type Frame } from "../types";
import { rulesFor, type ViewRules } from "../vision";
import { api, type PlayUnit, type Setup, type State } from "./api";
import { heldInput, heldKey, keyAction, NOTHING_HELD, sameHeld, toWorld, type Action, type Held, type InputMsg } from "./controls";
import { buildMenus, nextAlive, updateHud, updateSquad } from "./hud";
import { blend, interpFrom, tickPeriod } from "./smooth";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const canvas = $<HTMLCanvasElement>("canvas");
const ctx = canvas.getContext("2d")!;
const stage = $("stage");

let setup: Setup;
let map: GameMap;
let rules: ViewRules;
let palette: Palette;
let state: State | null = null;
/** The frame of the tick before `state`'s, drawn moving towards it, and when `state`'s tick arrived. */
let prev: Frame | null = null;
let arrived = 0;
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
/** Round end and detonation events of the round being played, and the round they belong to. */
let roundEvents: EventFrame[] = [];
let eventsRound = -1;
/** How the match ended, once it has, and when the page first showed that. */
let ending: Ending | null = null;
let endedAt = 0;

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

/** Unit id of the possessed unit, or null while watching. */
function possessedId(): number | null {
  return state?.possessed == null ? null : setup.first_unit + state.possessed;
}

/** The camera centred where the possessed unit is now (not where it is drawn): what aiming is relative to. */
function aimCamera(): Camera {
  const me = state?.view.me;
  return me && me.state & S_ALIVE ? { ...camera, cx: me.x / TILE, cy: me.y / TILE } : camera;
}

function draw(): void {
  raf = 0;
  if (!state) return;
  const cur = displayed(state.view.frame);
  const t = prev ? blend(performance.now() - arrived, tickPeriod(setup.tps)) : 1;
  // Between ticks: the previous tick moving towards the latest. Once there, the latest as it is.
  const from = prev && t < 1 ? displayed(interpFrom(prev, state.view.frame)) : null;
  const shown = from ?? cur;
  // The ending (headline, blast) grows in over its first ticks and then stays: the match is over.
  const endingT = ending ? Math.min(1, (performance.now() - endedAt) / (ENDING_TICKS * tickPeriod(setup.tps))) : 0;
  const id = possessedId();
  const me = state.view.me;
  if (id !== null && me && me.state & S_ALIVE) {
    const pose = interpUnits([shown.frame.units[id]], from ? [cur.frame.units[id]] : null, t)[0];
    camera = { ...camera, cx: pose.x / TILE, cy: pose.y / TILE };
  } else if (camera.cx === 0 && camera.cy === 0) {
    camera = { ...camera, cx: map.width / 2, cy: map.height / 2 };
  }
  const view = viewOf(shown.frame, { kind: "team", team: setup.team }, map, rules);
  view.unitVisible = view.unitVisible.map((v, i) => v && !shown.hidden[i]);
  // The server blanks the charge when the team does not know where it is.
  if (shown.frame.charge.state < 0) view.chargeVisible = false;
  render(ctx, width, height, dpr, {
    map,
    rules,
    frame: shown.frame,
    next: from ? cur.frame : null,
    t: from ? t : 0,
    view,
    camera,
    palette,
    show: { vision: true, debug: false, sounds: true },
    selected: id,
    marks: ending?.blast ? (pen) => drawBlast(pen, ending!.blast!, rules.blast_radius, endingT) : undefined,
  });
  if (ending) {
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    drawHeadline(ctx, width, height, ending, setup.bots, palette);
  }
  if (from || (ending && endingT < 1)) raf = requestAnimationFrame(draw);
}

/**
 * Keep the round's ending events (they arrive in one tick's frame only) and work out the ending once the
 * match is over. The page draws it itself; nothing of it goes to the server or the replay.
 */
function noteEnding(frame: Frame): void {
  if (frame.round !== eventsRound) {
    eventsRound = frame.round;
    roundEvents = [];
  }
  roundEvents.push(...frame.events.filter((e) => e.kind === EVENT_ROUND_END || e.kind === EVENT_DETONATE));
  if (!ending) {
    ending = endingOf(frame, roundEvents);
    if (ending) endedAt = performance.now();
  }
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
  // Answers can cross (a long poll and a step): never go back to an older tick.
  if (state && s.tick < state.tick) return;
  const newTick = state !== null && s.tick !== state.tick;
  if (newTick) {
    prev = state!.view.frame;
    arrived = performance.now();
  }
  state = s;
  noteEnding(s.view.frame);
  for (const u of s.view.frame.units as PlayUnit[]) {
    if (u.alive) deaths.delete(u.id);
    else if (!u.hidden) deaths.set(u.id, [u.x, u.y]);
  }
  updateHud(setup, rules, s);
  updateSquad(setup, s, possess);
  $("btn-run").textContent =
    setup.mode === "active" ? (s.running ? "Stop time (P)" : "Let time run (P)") : s.running ? "Pause (Space)" : "Start (Space)";
  // The unit moved, so the world point under a still mouse moved: re-aim.
  if (newTick) pushHeld();
  invalidate();
}

function report(e: unknown): void {
  $("error").textContent = (e as Error).message;
  $("error").hidden = false;
}

/** The world point under the mouse, if the mouse is over the map. */
function aim(): [number, number] | null {
  if (!mouse) return null;
  const [tx, ty] = toTiles(aimCamera(), mouse.x, mouse.y, width, height);
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
    .then(() => (setup.mode === "realtime" ? undefined : api.state().then(show)))
    .catch(report);
}

/** One tick now (turn mode: the only way; active mode: Space). */
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

function possess(slot: number | null): void {
  enqueue(() => api.possess(slot))
    .then(show)
    .catch(report);
}

function onSpace(e: KeyboardEvent): void {
  e.preventDefault();
  if (setup.mode === "turn") return void step();
  if (e.repeat) return;
  if (setup.mode === "realtime") return toggleRunning();
  // Active mode: a press is one tick at once; held, time keeps passing at the clock's rate.
  held = { ...held, wait: true };
  void step();
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
    if (e.code === "Space") return onSpace(e);
    if (e.repeat) return;
    if (e.code === "KeyP" && setup.mode !== "turn") return toggleRunning();
    if (e.code === "Tab") {
      e.preventDefault();
      if (state) possess(nextAlive(setup, state));
      return;
    }
    const a = state?.view.me ? keyAction(e.code, state.view.match.phase) : null;
    if (a) {
      e.preventDefault();
      sendAction(a);
    }
  });
  window.addEventListener("keyup", (e) => {
    const h = e.code === "Space" ? "wait" : heldKey(e.code);
    if (!h || !held[h]) return;
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

/** Follow the match: each answer comes as soon as a newer tick has run, until the match is over. */
async function follow(): Promise<void> {
  for (;;) {
    let s: State;
    try {
      s = await api.state(state?.tick);
    } catch {
      report(new Error("ibc play is not running any more: start it again to play"));
      return;
    }
    show(s);
    if (s.done) return;
  }
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
  if (typeof setup.first_unit !== "number" || typeof setup.team_size !== "number") {
    throw new Error("this ibc is older than its page: reinstall it (cargo install --path crates/cli) and restart");
  }
  map = parseMap(setup.map_text);
  rules = rulesFor(setup.rules);
  $("match-title").textContent = `${setup.bots[0]} vs ${setup.bots[1]} · ${map.name} · team ${"AB"[setup.team]} (${setup.mode})`;
  $("btn-step").hidden = setup.mode === "realtime";
  $("btn-step").textContent = setup.mode === "active" ? "Wait (Space)" : "Step (Space)";
  $("btn-run").hidden = setup.mode === "turn";
  buildMenus(setup, sendAction);
  bindKeys();
  bindMouse();
  bindButtons();
  new ResizeObserver(resize).observe(stage);
  resize();
  void follow();
}

// A handle for scripted checks (end-to-end tests drive the page through it).
Object.assign(window, {
  ibcPlay: {
    state: () => state,
    step,
    send: sendAction,
    possess,
  },
});

start().catch(report);
