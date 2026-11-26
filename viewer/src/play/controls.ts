// Keyboard and mouse -> the input messages `ibc play` takes (`POST /input`, `POST /step`). Pure functions,
// so the mapping is unit-tested. The server turns these into the acts a bot would make, and the engine
// validates them exactly as it validates a bot's: nothing here can do what a bot cannot.

import { PHASE_ACTION, PHASE_BUY, PHASE_DRAFT, PHASE_PLANTED, TILE } from "../types";

/** A one-shot action, as `crates/cli/src/play/input.rs` reads it. */
export type Action =
  | { act: "reload" }
  | { act: "ability"; slot: number }
  | { act: "buy"; item: number }
  | { act: "draft"; kit: number }
  | { act: "swap"; slot: number }
  | { act: "pickup" }
  | { act: "cancel" };

/** Held controls plus queued actions. Every field is optional on the wire. */
export interface InputMsg {
  move?: [number, number];
  walk?: boolean;
  aim?: [number, number] | null;
  fire?: boolean;
  interact?: boolean;
  actions?: Action[];
}

/** What is held down right now. */
export interface Held {
  up: boolean;
  down: boolean;
  left: boolean;
  right: boolean;
  walk: boolean;
  fire: boolean;
  interact: boolean;
}

export const NOTHING_HELD: Held = {
  up: false,
  down: false,
  left: false,
  right: false,
  walk: false,
  fire: false,
  interact: false,
};

/** Keys that are held rather than pressed, by `KeyboardEvent.code` (layout independent: WASD stays put). */
const HELD_KEYS: Record<string, keyof Held> = {
  KeyW: "up",
  KeyS: "down",
  KeyA: "left",
  KeyD: "right",
  ArrowUp: "up",
  ArrowDown: "down",
  ArrowLeft: "left",
  ArrowRight: "right",
  ShiftLeft: "walk",
  ShiftRight: "walk",
  KeyF: "interact",
};

/** The held control a key code drives, if any. */
export function heldKey(code: string): keyof Held | null {
  return HELD_KEYS[code] ?? null;
}

/** Screen-axis move direction (+x right, +y down); opposite keys cancel. */
export function moveVector(h: Held): [number, number] {
  return [(h.right ? 1 : 0) - (h.left ? 1 : 0), (h.down ? 1 : 0) - (h.up ? 1 : 0)];
}

/** The held part of an input message. */
export function heldInput(h: Held, aim: [number, number] | null): InputMsg {
  return { move: moveVector(h), walk: h.walk, aim, fire: h.fire, interact: h.interact };
}

/** Whether two held messages differ (so an unchanged one is not sent again). */
export function sameHeld(a: InputMsg, b: InputMsg): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/**
 * The one-shot action a pressed key means in this phase, or null. Digits draft a kit in the draft phase and
 * buy an item in the buy phase (1 = the first, 0 = the tenth). In the action phases: R reload, Q / E the
 * abilities in slot 0 / 1, 1 / 2 switch weapon slot, G pick up, C cancel the current channel.
 */
export function keyAction(code: string, phase: number): Action | null {
  const digit = /^Digit(\d)$/.exec(code);
  const index = digit ? (Number(digit[1]) + 9) % 10 : -1;
  if (phase === PHASE_DRAFT) return index >= 0 ? { act: "draft", kit: index } : null;
  if (phase === PHASE_BUY) return index >= 0 ? { act: "buy", item: index } : null;
  if (phase !== PHASE_ACTION && phase !== PHASE_PLANTED) return null;
  if (index === 0 || index === 1) return { act: "swap", slot: index };
  const keys: Record<string, Action> = {
    KeyR: { act: "reload" },
    KeyQ: { act: "ability", slot: 0 },
    KeyE: { act: "ability", slot: 1 },
    KeyG: { act: "pickup" },
    KeyC: { act: "cancel" },
  };
  return keys[code] ?? null;
}

/** A point in tiles (the camera's units) as the engine's fixed-point world position. */
export function toWorld(tx: number, ty: number): [number, number] {
  return [Math.round(tx * TILE), Math.round(ty * TILE)];
}
