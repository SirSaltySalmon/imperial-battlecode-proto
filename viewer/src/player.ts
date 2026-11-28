// Plays a list of frames on a canvas the way the replay viewer does: the clock and the in-between positions,
// perspectives and layers, the camera, pointer and keys. The replay viewer and the combat lab each drive one;
// what a page adds (panels, files, rounds) it does through `PlayerHooks` and its own key table.

import { fitCamera, pan, toTiles, zoomAt, type Camera } from "./camera";
import { interpUnits } from "./interp";
import type { GameMap } from "./map";
import { ALL, viewOf, type Perspective } from "./perspective";
import { render, type Pen } from "./render";
import type { Palette } from "./theme";
import { TILE, type Frame } from "./types";
import type { ViewRules } from "./vision";

/** Ticks per second at 1x. */
export const BASE_TPS = 8;
export const SPEEDS = [0.25, 0.5, 1, 2, 4, 8, 16];

/** What a player plays: frames by index, and the map and rules to draw them with. */
export interface Track {
  count: number;
  frame(i: number): Frame;
  map: GameMap;
  rules: ViewRules;
  /** Units per team, for unit views. */
  teamSize: number;
  /**
   * Drawing on top of the frame, in tile coordinates (the lab's fan marks). `pos` is the playback position and
   * `held` the ticks playback has held on it so far (see `hold`).
   */
  marks?: (pen: Pen, i: number, pos: number, held: number) => void;
  /** Drawing on top of everything, in CSS pixels. */
  overlay?: (ctx: CanvasRenderingContext2D, width: number, height: number, pos: number, held: number) => void;
  /** Ticks playback holds on frame `i` before moving on (a round's ending is drawn over the held frame). */
  hold?: (i: number) => number;
  /** Camera to fit to; the default is the whole map. */
  fit?: (width: number, height: number) => Camera;
}

export interface PlayerHooks {
  palette(): Palette;
  /** The frame shown changed, or something the page shows did (perspective, speed, layer). */
  page(i: number, frame: Frame): void;
  /** Playback started or stopped. */
  playing(on: boolean): void;
}

export type Layer = "vision" | "sounds" | "debug";

export class Player {
  track: Track | null = null;
  /** Ticks of playback spent so far holding on the whole frame `pos` (`Track.hold`). */
  held = 0;
  /** Playback position in frames; the fraction is the way towards the next frame. */
  pos = 0;
  playing = false;
  speed = 1;
  perspective: Perspective = ALL;
  readonly show = { vision: true, sounds: true, debug: true };
  camera: Camera = { cx: 0, cy: 0, ppt: 24 };
  /** Until the user pans or zooms, the camera is refitted whenever the stage changes size. */
  cameraIsAuto = true;
  width = 1;
  height = 1;
  private dpr = 1;
  private raf = 0;
  private lastTime = 0;
  /** Frame index the page was last updated for; -1 forces an update. */
  private pageFrame = -1;
  private readonly ctx: CanvasRenderingContext2D;

  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly stage: HTMLElement,
    private readonly hooks: PlayerHooks,
  ) {
    this.ctx = canvas.getContext("2d")!;
  }

  /** Start playing `track` from its first frame, stopped, seen by everyone, fitted. */
  load(track: Track | null): void {
    this.track = track;
    this.pos = 0;
    this.held = 0;
    this.perspective = ALL;
    this.setPlaying(false);
    this.fit();
    this.invalidate(true);
  }

  invalidate(page = false): void {
    if (page) this.pageFrame = -1;
    if (!this.raf) this.raf = requestAnimationFrame((now) => this.onFrame(now));
  }

  /** The frame under the playback position, the next one, and how far playback is between them. */
  moment(): { i: number; t: number; frame: Frame; next: Frame | null } | null {
    const tr = this.track;
    if (!tr) return null;
    const i = Math.max(0, Math.min(Math.floor(this.pos), tr.count - 1));
    const next = this.pos > i && i + 1 < tr.count ? tr.frame(i + 1) : null;
    return { i, t: next ? this.pos - i : 0, frame: tr.frame(i), next };
  }

  setPlaying(on: boolean): void {
    const tr = this.track;
    if (on && (!tr || tr.count < 2)) on = false;
    if (on && tr && this.pos >= tr.count - 1) {
      this.pos = 0;
      this.held = 0;
    }
    this.playing = on;
    this.lastTime = performance.now();
    this.hooks.playing(on);
    this.invalidate();
  }

  seek(frame: number): void {
    if (!this.track) return;
    this.pos = Math.max(0, Math.min(this.track.count - 1, Math.round(frame)));
    this.held = 0;
    this.invalidate();
  }

  /** Step whole ticks from wherever playback is, pausing first. */
  step(delta: number): void {
    this.setPlaying(false);
    const whole = Math.floor(this.pos);
    this.seek(delta < 0 && this.pos > whole ? whole + delta + 1 : whole + delta);
  }

  setSpeed(value: number): void {
    this.speed = SPEEDS.includes(value) ? value : 1;
    this.invalidate(true);
  }

  /** One step along `SPEEDS`. */
  nudgeSpeed(delta: number): void {
    const at = SPEEDS.indexOf(this.speed);
    this.setSpeed(SPEEDS[Math.max(0, Math.min(SPEEDS.length - 1, at + delta))]);
  }

  setPerspective(p: Perspective): void {
    // A unit id the frames do not have (a stale link) means everyone, not a view that claims to be fogged.
    const known = p.kind !== "unit" || (this.track !== null && p.id < this.track.teamSize * 2);
    this.perspective = known ? p : ALL;
    this.invalidate(true);
  }

  cycleUnit(delta: number): void {
    if (!this.track) return;
    const n = this.track.teamSize * 2;
    const p = this.perspective;
    const cur = p.kind === "unit" ? p.id : delta > 0 ? -1 : n;
    this.setPerspective({ kind: "unit", id: (cur + delta + n) % n });
  }

  toggleLayer(layer: Layer): void {
    this.show[layer] = !this.show[layer];
    this.invalidate(true);
  }

  fit(): void {
    const tr = this.track;
    if (tr) {
      this.camera = tr.fit ? tr.fit(this.width, this.height) : fitCamera(tr.map.width, tr.map.height, this.width, this.height);
    }
    this.cameraIsAuto = true;
    this.invalidate();
  }

  resize(): void {
    const box = this.stage.getBoundingClientRect();
    this.width = Math.max(1, box.width);
    this.height = Math.max(1, box.height);
    this.dpr = window.devicePixelRatio || 1;
    this.canvas.width = Math.round(this.width * this.dpr);
    this.canvas.height = Math.round(this.height * this.dpr);
    if (this.cameraIsAuto) this.fit();
    this.invalidate();
  }

  /** Keys every page that plays frames shares. A page merges its own over these. */
  keys(e: KeyboardEvent): Record<string, () => void> {
    return {
      " ": () => this.setPlaying(!this.playing),
      k: () => this.setPlaying(!this.playing),
      ArrowLeft: () => this.step(e.shiftKey ? -10 : -1),
      ArrowRight: () => this.step(e.shiftKey ? 10 : 1),
      ArrowUp: () => this.nudgeSpeed(1),
      ArrowDown: () => this.nudgeSpeed(-1),
      Home: () => this.seek(0),
      End: () => this.seek(Infinity),
      "1": () => this.setPerspective(ALL),
      "2": () => this.setPerspective({ kind: "team", team: 0 }),
      "3": () => this.setPerspective({ kind: "team", team: 1 }),
      Escape: () => this.setPerspective(ALL),
      ".": () => this.cycleUnit(1),
      ",": () => this.cycleUnit(-1),
      v: () => this.toggleLayer("vision"),
      s: () => this.toggleLayer("sounds"),
      d: () => this.toggleLayer("debug"),
      f: () => this.fit(),
    };
  }

  /** Listen for keys: the shared table with `extra(e)` over it. */
  bindKeys(extra: (e: KeyboardEvent) => Record<string, () => void> = () => ({})): void {
    window.addEventListener("keydown", (e) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      // A focused control keeps the keys it uses itself: Space / Enter press a button, arrows move a slider or
      // a select. Everything else stays a shortcut.
      const tag = (e.target as HTMLElement).tagName;
      const own: Record<string, string[]> = {
        BUTTON: [" ", "Enter"],
        SELECT: [" ", "Enter", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End"],
        INPUT: ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End"],
        TEXTAREA: [],
      };
      if (own[tag]?.includes(e.key)) return;
      const actions = { ...this.keys(e), ...extra(e) };
      const action = actions[e.key.length === 1 ? e.key.toLowerCase() : e.key];
      if (!action) return;
      e.preventDefault();
      action();
    });
  }

  /** Pan by dragging, zoom by wheel or pinch, click a unit to see through it, double-click to fit. */
  bindPointer(): void {
    const canvas = this.canvas;
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
          this.camera = pan(this.camera, now.x - before.x, now.y - before.y);
          this.cameraIsAuto = false;
        }
      } else if (pointers.size === 2) {
        // Pinch: zoom about the midpoint by the change in finger distance.
        const other = [...pointers.entries()].find(([id]) => id !== e.pointerId)![1];
        const d0 = Math.hypot(before.x - other.x, before.y - other.y);
        const d1 = Math.hypot(now.x - other.x, now.y - other.y);
        if (d0 > 0) {
          this.camera = zoomAt(this.camera, d1 / d0, (now.x + other.x) / 2, (now.y + other.y) / 2, this.width, this.height);
        }
        this.cameraIsAuto = false;
        moved = 99;
      }
      pointers.set(e.pointerId, now);
      this.invalidate();
    });
    const release = (e: PointerEvent) => {
      if (!pointers.delete(e.pointerId)) return;
      if (pointers.size === 0) canvas.classList.remove("panning");
      if (e.type === "pointerup" && moved <= 4) {
        const { x, y } = local(e);
        const id = this.unitAt(x, y);
        if (id !== null) this.setPerspective({ kind: "unit", id });
      }
    };
    canvas.addEventListener("pointerup", release);
    canvas.addEventListener("pointercancel", release);
    canvas.addEventListener(
      "wheel",
      (e) => {
        e.preventDefault();
        const { x, y } = local(e);
        this.camera = zoomAt(this.camera, Math.exp(-e.deltaY * 0.0015), x, y, this.width, this.height);
        this.cameraIsAuto = false;
        this.invalidate();
      },
      { passive: false },
    );
    canvas.addEventListener("dblclick", (e) => {
      const { x, y } = local(e);
      if (this.unitAt(x, y) === null) this.fit();
    });
  }

  /** The unit drawn under a viewport point, if any (only units the perspective shows). */
  unitAt(px: number, py: number): number | null {
    const tr = this.track;
    const m = this.moment();
    if (!tr || !m) return null;
    // Hit-test what is on screen: the same in-between poses the canvas draws.
    const poses = interpUnits(m.frame.units, m.next?.units ?? null, m.t);
    const visible = viewOf(m.frame, this.perspective, tr.map, tr.rules).unitVisible;
    const [tx, ty] = toTiles(this.camera, px, py, this.width, this.height);
    const reach = Math.max((tr.rules.unit_radius / TILE) * 1.5, 10 / this.camera.ppt);
    let best: number | null = null;
    let bestD = reach;
    m.frame.units.forEach((u, i) => {
      const d = Math.hypot(poses[i].x / TILE - tx, poses[i].y / TILE - ty);
      if (u.alive && visible[i] && d <= bestD) {
        best = u.id;
        bestD = d;
      }
    });
    return best;
  }

  /**
   * Move playback on by `ticks`, holding on frames the track asks to hold on. False once it is at the final
   * frame with any hold there done.
   */
  advance(ticks: number): boolean {
    const tr = this.track;
    if (!tr) return false;
    let left = ticks;
    for (;;) {
      const whole = this.pos === Math.floor(this.pos);
      const hold = whole ? (tr.hold?.(this.pos) ?? 0) : 0;
      if (this.held < hold) {
        const take = Math.min(left, hold - this.held);
        this.held += take;
        left -= take;
        if (this.held < hold) return true;
        continue;
      }
      const next = Math.floor(this.pos) + 1;
      if (next > tr.count - 1) return false;
      if (left <= 0) return true;
      const move = Math.min(left, next - this.pos);
      this.pos += move;
      left -= move;
      if (this.pos >= next) {
        this.pos = next;
        this.held = 0;
      }
    }
  }

  private onFrame(now: number): void {
    this.raf = 0;
    const tr = this.track;
    if (this.playing && tr) {
      // A background tab can stall for seconds; do not jump forward when it comes back. The timestamp of a
      // frame can also lie slightly before the input event that started playback.
      if (!this.advance((Math.max(0, Math.min(now - this.lastTime, 100)) / 1000) * BASE_TPS * this.speed)) {
        this.setPlaying(false);
      }
    }
    this.lastTime = now;
    this.draw();
    if (this.playing) this.invalidate();
  }

  private draw(): void {
    const tr = this.track;
    const m = this.moment();
    const palette = this.hooks.palette();
    if (!tr || !m) {
      this.ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
      this.ctx.fillStyle = palette.bg;
      this.ctx.fillRect(0, 0, this.width, this.height);
      return;
    }
    const p = this.perspective;
    render(this.ctx, this.width, this.height, this.dpr, {
      map: tr.map,
      rules: tr.rules,
      frame: m.frame,
      next: m.next,
      t: m.t,
      view: viewOf(m.frame, p, tr.map, tr.rules),
      camera: this.camera,
      palette,
      show: this.show,
      selected: p.kind === "unit" ? p.id : null,
      marks: tr.marks ? (pen) => tr.marks!(pen, m.i, this.pos, this.held) : undefined,
    });
    if (tr.overlay) {
      this.ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
      tr.overlay(this.ctx, this.width, this.height, this.pos, this.held);
    }
    if (m.i !== this.pageFrame) {
      this.pageFrame = m.i;
      this.hooks.page(m.i, m.frame);
    }
  }
}
