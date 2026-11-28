// Page wiring: loading a replay, the page around the player (panels, rounds, library), and the controls.

import { BotRecordsPanel, parseRecords } from "./bot-records";
import { drawBlast, drawHeadline, endingAt, holdAt, roundEndings } from "./ending";
import { fetchEngine, FrameStore } from "./engine";
import { Panels } from "./panels";
import { readHeader } from "./ibcr";
import { currentIndex, entryHref, entryLabel, fetchLibrary, neighbour, type LibraryEntry } from "./library";
import { parseMap, type GameMap } from "./map";
import { formatPerspective, parsePerspective } from "./perspective";
import { Player, SPEEDS } from "./player";
import { applyTheme, type Palette, type ThemeName } from "./theme";
import { buildTimeline, roundIndexAt, type Timeline } from "./timeline";
import type { Frame, Header } from "./types";
import { buildMarkers, Panel, updateBanner } from "./ui";
import { rulesFor, type ViewRules } from "./vision";

interface Loaded {
  header: Header;
  store: FrameStore;
  timeline: Timeline;
  map: GameMap;
  rules: ViewRules;
  panel: Panel;
  bot: BotRecordsPanel;
  teamSize: number;
}

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const canvas = $<HTMLCanvasElement>("canvas");
const stage = $("stage");
const scrub = $<HTMLInputElement>("scrub");
const speedSelect = $<HTMLSelectElement>("speed");
const roundSelect = $<HTMLSelectElement>("round-select");
const playButton = $<HTMLButtonElement>("btn-play");
const fileInput = $<HTMLInputElement>("file");
const panels = new Panels({ buy: $("buy"), tab: $("tab"), feed: $("feed") });

let loaded: Loaded | null = null;
let themeName: ThemeName = "dark";
let palette: Palette;
let loadToken = 0;
/** The replay folder the server offers, and which of its entries is open (-1: none of them). */
let library: LibraryEntry[] = [];
let libraryAt = -1;

const player = new Player(canvas, stage, {
  palette: () => palette,
  page: updatePage,
  playing: (on) => (playButton.textContent = on ? "Pause" : "Play"),
});

/** Everything on the page that follows the frame shown: side panel, feed, banner, scrub bar, readout. */
function updatePage(i: number, frame: Frame): void {
  if (!loaded) return;
  loaded.panel.update(frame, player.perspective, player.show);
  panels.update(i, frame);
  loaded.bot.update(frame.tick, player.perspective);
  updateBanner($("banner"), loaded.timeline, i, frame, loaded.header, loaded.map);
  scrub.value = String(i);
  roundSelect.value = String(roundIndexAt(loaded.timeline, i));
  speedSelect.value = String(player.speed);
  $("readout").textContent = `tick ${frame.tick} / ${loaded.store.count - 1} · ${player.speed}x`;
}

function jumpRound(delta: number): void {
  if (!loaded) return;
  const { rounds } = loaded.timeline;
  const at = roundIndexAt(loaded.timeline, Math.floor(player.pos));
  // "Previous" first returns to the start of the current round, like a music player.
  const target = delta < 0 && Math.floor(player.pos) > rounds[at].start ? at : at + delta;
  player.seek(rounds[Math.max(0, Math.min(rounds.length - 1, target))].start);
}

function setTheme(name: ThemeName): void {
  themeName = name;
  palette = applyTheme(name);
  $("btn-theme").textContent = name === "dark" ? "Light" : "Dark";
  player.invalidate();
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
  player.setPlaying(false);
  setStatus(`Opening ${name}...`);
  let engineTouched = false;
  try {
    const peek = await readHeader(bytes);
    const engine = await fetchEngine(peek.engine_version);
    if (token !== loadToken) return;
    // From here the engine drops the frames of the replay that was open, so that one is gone either way.
    engineTouched = true;
    loaded = null;
    player.load(null);
    $("empty").hidden = false;
    const { header, frames } = engine.load(bytes);
    const records = parseRecords(engine.records());
    const store = new FrameStore(frames, undefined, records.draws);
    const map = parseMap(header.map_text);
    const rules = rulesFor(engine.rules());
    const first = store.get(0);
    const teamSize = first.units.length / 2;
    const panel = new Panel($("panel"), header, map, first, rules, {
      setPerspective: (p) => player.setPerspective(p),
      toggle: (layer) => player.toggleLayer(layer),
    });
    const bot = new BotRecordsPanel(records, teamSize);
    $("panel").append(bot.root);
    loaded = { header, store, timeline: buildTimeline(store), map, rules, panel, bot, teamSize };
    const strip = document.createElement("div");
    $("panel").querySelector(".phase")?.after(strip);
    panels.load({ ...loaded, view: rules, rawRules: engine.rawRules() }, strip);
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
    panels.clear();
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
  const { store, map, rules, teamSize, header } = loaded;
  const endings = roundEndings(loaded.timeline, (i) => store.get(i).score);
  const names: [string, string] = [header.bots[0].name, header.bots[1].name];
  player.load({
    count: store.count,
    frame: (i) => store.get(i),
    map,
    rules,
    teamSize,
    // Round endings are the viewer's own: shown from the round-end event, held for a few ticks on the round's
    // last frame.
    hold: (i) => holdAt(endings, i),
    marks: (pen, i, pos, held) => {
      const e = endingAt(endings, i);
      if (e?.blast) drawBlast(pen, e.blast, rules.blast_radius, pos + held - e.from);
    },
    overlay: (ctx, width, height, pos) => {
      const e = endingAt(endings, Math.floor(pos));
      if (e) drawHeadline(ctx, width, height, e, names, palette);
    },
  });
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

/** The replay viewer's own keys, over the player's. */
function pageKeys(): Record<string, () => void> {
  return {
    n: () => jumpRound(1),
    p: () => jumpRound(-1),
    t: toggleTheme,
    o: () => fileInput.click(),
    "[": () => openNeighbour(-1),
    "]": () => openNeighbour(1),
    "?": () => ($("help").hidden = !$("help").hidden),
  };
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
  speedSelect.addEventListener("change", () => player.setSpeed(Number(speedSelect.value)));
  playButton.addEventListener("click", () => player.setPlaying(!player.playing));
  $("btn-step").addEventListener("click", () => player.step(1));
  $("btn-step-back").addEventListener("click", () => player.step(-1));
  $("btn-next-round").addEventListener("click", () => jumpRound(1));
  $("btn-prev-round").addEventListener("click", () => jumpRound(-1));
  scrub.addEventListener("input", () => {
    player.setPlaying(false);
    player.seek(Number(scrub.value));
  });
  roundSelect.addEventListener("change", () => {
    if (loaded) player.seek(loaded.timeline.rounds[Number(roundSelect.value)].start);
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

/** Open the replay `delta` places along the folder (a page load, so the address always names the replay). */
function openNeighbour(delta: number): void {
  const e = neighbour(library, libraryAt, delta);
  if (e) location.search = entryHref(e);
}

/** Fill the header menu and the list on the empty page from the folder the server offers, if any. */
async function loadLibrary(replayParam: string | null): Promise<void> {
  library = await fetchLibrary();
  if (library.length === 0) return;
  libraryAt = currentIndex(library, replayParam);
  const select = $<HTMLSelectElement>("library-select");
  select.replaceChildren(new Option(`Replays (${library.length})`, "", libraryAt < 0, libraryAt < 0));
  const list = $("library-list");
  library.forEach((e, i) => {
    select.add(new Option(entryLabel(e), String(i), false, i === libraryAt));
    const link = document.createElement("a");
    link.href = entryHref(e);
    link.textContent = entryLabel(e);
    const item = document.createElement("li");
    item.append(link);
    list.append(item);
  });
  select.addEventListener("change", () => {
    const e = library[Number(select.value)];
    if (select.value !== "" && e) location.search = entryHref(e);
  });
  $("btn-prev-replay").addEventListener("click", () => openNeighbour(-1));
  $("btn-next-replay").addEventListener("click", () => openNeighbour(1));
  $("library").hidden = false;
  list.hidden = false;
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
  player.bindPointer();
  player.bindKeys(pageKeys);
  panels.bindKeys();
  new ResizeObserver(() => player.resize()).observe(stage);
  player.resize();
  const replay = params.get("replay");
  void loadLibrary(replay);
  if (!replay) return;
  await openUrl(replay);
  if (!loaded) return;
  if (params.has("speed")) player.setSpeed(Number(params.get("speed")));
  if (params.has("view")) player.setPerspective(parsePerspective(params.get("view")));
  if (params.has("tick")) player.seek(Number(params.get("tick")) || 0);
  if (params.get("play") === "1") player.setPlaying(true);
}

// A handle for scripted checks (end-to-end tests drive the viewer through it).
Object.assign(window, {
  ibcViewer: {
    seek: (frame: number) => player.seek(frame),
    setPlaying: (on: boolean) => player.setPlaying(on),
    setSpeed: (value: number) => player.setSpeed(value),
    setPerspective: (text: string) => player.setPerspective(parsePerspective(text)),
    /** Jump to a fractional frame position, to look at an interpolated moment while paused. */
    setPosition(p: number) {
      if (!loaded) return;
      player.pos = Math.max(0, Math.min(loaded.store.count - 1, p));
      player.invalidate();
    },
    state: () => ({
      loaded: !!loaded,
      frames: loaded?.store.count ?? 0,
      pos: player.pos,
      playing: player.playing,
      speed: player.speed,
      perspective: formatPerspective(player.perspective),
      rounds: loaded?.timeline.rounds ?? [],
      cachedFrames: loaded?.store.cached ?? 0,
    }),
  },
});

void start();
