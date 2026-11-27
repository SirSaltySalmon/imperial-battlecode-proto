// Keyboard and mouse -> the input messages `ibc play` takes (`POST /input`, `POST /step`). Pure functions,
// so the mapping is unit-tested. The server turns these into the acts a bot would make, and the engine
// validates them exactly as it validates a bot's: nothing here can do what a bot cannot.

import {
  CHARGE_PLANTED,
  DROP_CHARGE,
  DROP_WEAPON,
  PHASE_ACTION,
  PHASE_BUY,
  PHASE_PLANTED,
  S_ALIVE,
  S_DEFUSING,
  S_HAS_CHARGE,
  S_PLANTING,
  SIDE_ATTACK,
  TILE,
} from "../types";

/** A one-shot action, as `crates/cli/src/play/input.rs` reads it. */
export type Action =
  | { act: "reload" }
  | { act: "ability"; slot: number }
  | { act: "buy"; item: number }
  | { act: "swap"; slot: number }
  | { act: "drop"; kind: number }
  | { act: "cancel" };

/** Held controls plus queued actions. Every field is optional on the wire. */
export interface InputMsg {
  move?: [number, number];
  walk?: boolean;
  aim?: [number, number] | null;
  fire?: boolean;
  interact?: boolean;
  wait?: boolean;
  actions?: Action[];
}

/** What is held down right now. `wait`: Space in active mode, letting time pass. */
export interface Held {
  up: boolean;
  down: boolean;
  left: boolean;
  right: boolean;
  walk: boolean;
  fire: boolean;
  interact: boolean;
  wait: boolean;
}

export const NOTHING_HELD: Held = {
  up: false,
  down: false,
  left: false,
  right: false,
  walk: false,
  fire: false,
  interact: false,
  wait: false,
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
  return { move: moveVector(h), walk: h.walk, aim, fire: h.fire, interact: h.interact, wait: h.wait };
}

/** Whether two held messages differ (so an unchanged one is not sent again). */
export function sameHeld(a: InputMsg, b: InputMsg): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/**
 * The one-shot action a pressed key means in this phase, or null. Digits buy an item in the buy phase (1 =
 * the first, 0 = the tenth). In the action phases: R reload, Q / E the abilities in slot 0 / 1, 1 / 2 switch
 * weapon slot, G drop the weapon in hand, X drop the charge, C cancel the current channel (F, held, picks
 * up). The draft is the team's bot's.
 */
export function keyAction(code: string, phase: number): Action | null {
  const digit = /^Digit(\d)$/.exec(code);
  const index = digit ? (Number(digit[1]) + 9) % 10 : -1;
  if (phase === PHASE_BUY) return index >= 0 ? { act: "buy", item: index } : null;
  if (phase !== PHASE_ACTION && phase !== PHASE_PLANTED) return null;
  if (index === 0 || index === 1) return { act: "swap", slot: index };
  const keys: Record<string, Action> = {
    KeyR: { act: "reload" },
    KeyQ: { act: "ability", slot: 0 },
    KeyE: { act: "ability", slot: 1 },
    KeyG: { act: "drop", kind: DROP_WEAPON },
    KeyX: { act: "drop", kind: DROP_CHARGE },
    KeyC: { act: "cancel" },
  };
  return keys[code] ?? null;
}

/**
 * What `A_PICKUP` would take for this unit, as the engine chooses (`World::pickup`): the charge if an attacker
 * is in reach of it, else the nearest weapon in reach; null if nothing is.
 */
export function pickupTarget<D extends { kind: number; x: number; y: number }>(
  me: { x: number; y: number },
  drops: D[],
  reach: number,
  attacking: boolean,
): D | null {
  let best: D | null = null;
  let bestKey = Infinity;
  for (const d of drops) {
    if (d.kind === DROP_CHARGE && !attacking) continue;
    if (d.kind !== DROP_CHARGE && d.kind !== DROP_WEAPON) continue;
    const dist = Math.hypot(d.x - me.x, d.y - me.y);
    const key = d.kind === DROP_CHARGE ? -1 : dist;
    if (dist <= reach && key < bestKey) [best, bestKey] = [d, key];
  }
  return best;
}

/**
 * What F does for this unit right now, as a prompt, or null when it does nothing. A press picks up what is in
 * reach (`pickup`: its name), once; holding on plants (attackers) or defuses (defenders).
 */
export function interactHint(
  me: { state: number; channel: number } | null,
  match: { phase: number; my_side: number; charge_state: number },
  pickup: string | null = null,
): string | null {
  if (!me || !(me.state & S_ALIVE)) return null;
  if (match.phase !== PHASE_ACTION && match.phase !== PHASE_PLANTED) return null;
  if (me.state & S_DEFUSING) return `Defusing: keep holding F (${me.channel} ticks left)`;
  if (me.state & S_PLANTING) return `Planting: keep holding F (${me.channel} ticks left)`;
  if (pickup) return `Press F to pick up the ${pickup}`;
  const attacking = match.my_side === SIDE_ATTACK;
  if (!attacking && match.charge_state === CHARGE_PLANTED) return "Charge planted: hold F at the charge to defuse";
  if (attacking && me.state & S_HAS_CHARGE) return "You carry the charge: hold F on a site to plant";
  return null;
}

/** A point in tiles (the camera's units) as the engine's fixed-point world position. */
export function toWorld(tx: number, ty: number): [number, number] {
  return [Math.round(tx * TILE), Math.round(ty * TILE)];
}
