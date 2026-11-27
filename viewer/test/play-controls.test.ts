import { describe, expect, it } from "vitest";
import {
  heldInput,
  heldKey,
  interactHint,
  keyAction,
  moveVector,
  NOTHING_HELD,
  pickupTarget,
  sameHeld,
  toWorld,
} from "../src/play/controls";
import {
  CHARGE_PLANTED,
  DROP_CHARGE,
  DROP_WEAPON,
  PHASE_ACTION,
  PHASE_BUY,
  PHASE_DRAFT,
  PHASE_PLANTED,
  PHASE_ROUND_END,
  S_ALIVE,
  S_DEFUSING,
  S_HAS_CHARGE,
} from "../src/types";

describe("play controls", () => {
  it("maps WASD and the arrows to a move vector where opposite keys cancel", () => {
    expect(heldKey("KeyW")).toBe("up");
    expect(heldKey("ArrowLeft")).toBe("left");
    expect(heldKey("ShiftLeft")).toBe("walk");
    expect(heldKey("KeyF")).toBe("interact");
    expect(heldKey("KeyX")).toBeNull();
    expect(moveVector(NOTHING_HELD)).toEqual([0, 0]);
    expect(moveVector({ ...NOTHING_HELD, up: true, right: true })).toEqual([1, -1]);
    expect(moveVector({ ...NOTHING_HELD, left: true, right: true, down: true })).toEqual([0, 1]);
  });

  it("builds the held message and compares it", () => {
    const a = heldInput({ ...NOTHING_HELD, down: true, walk: true, fire: true }, [100, 200]);
    expect(a).toEqual({ move: [0, 1], walk: true, aim: [100, 200], fire: true, interact: false, wait: false });
    expect(heldInput({ ...NOTHING_HELD, wait: true }, null).wait).toBe(true);
    expect(sameHeld(a, heldInput({ ...NOTHING_HELD, down: true, walk: true, fire: true }, [100, 200]))).toBe(true);
    expect(sameHeld(a, heldInput({ ...NOTHING_HELD, down: true, walk: true, fire: true }, [101, 200]))).toBe(false);
    expect(heldInput(NOTHING_HELD, null).aim).toBeNull();
  });

  it("gives digits the meaning of the phase", () => {
    expect(keyAction("Digit1", PHASE_DRAFT)).toBeNull();
    expect(keyAction("Digit3", PHASE_BUY)).toEqual({ act: "buy", item: 2 });
    expect(keyAction("Digit0", PHASE_BUY)).toEqual({ act: "buy", item: 9 });
    expect(keyAction("Digit2", PHASE_ACTION)).toEqual({ act: "swap", slot: 1 });
    expect(keyAction("Digit5", PHASE_ACTION)).toBeNull();
    expect(keyAction("KeyR", PHASE_BUY)).toBeNull();
  });

  it("maps the action keys in both action phases only", () => {
    expect(keyAction("KeyR", PHASE_ACTION)).toEqual({ act: "reload" });
    expect(keyAction("KeyQ", PHASE_PLANTED)).toEqual({ act: "ability", slot: 0 });
    expect(keyAction("KeyE", PHASE_ACTION)).toEqual({ act: "ability", slot: 1 });
    expect(keyAction("KeyG", PHASE_ACTION)).toEqual({ act: "drop", kind: DROP_WEAPON });
    expect(keyAction("KeyX", PHASE_PLANTED)).toEqual({ act: "drop", kind: DROP_CHARGE });
    expect(keyAction("KeyC", PHASE_ACTION)).toEqual({ act: "cancel" });
    expect(keyAction("KeyR", PHASE_ROUND_END)).toBeNull();
  });

  it("prompts for F when it would plant or defuse", () => {
    const alive = { state: S_ALIVE, channel: 0 };
    const planted = { phase: PHASE_PLANTED, my_side: 1, charge_state: CHARGE_PLANTED };
    expect(interactHint(alive, planted)).toMatch(/hold F .*defuse/);
    expect(interactHint({ state: S_ALIVE | S_DEFUSING, channel: 7 }, planted)).toMatch(/7 ticks left/);
    expect(interactHint(alive, { ...planted, my_side: 0 })).toBeNull();
    expect(interactHint({ state: 0, channel: 0 }, planted)).toBeNull();
    expect(interactHint(null, planted)).toBeNull();
    const carrying = { state: S_ALIVE | S_HAS_CHARGE, channel: 0 };
    expect(interactHint(carrying, { phase: PHASE_ACTION, my_side: 0, charge_state: 0 })).toMatch(/hold F .*plant/);
    expect(interactHint(carrying, { phase: PHASE_BUY, my_side: 0, charge_state: 0 })).toBeNull();
    expect(interactHint(carrying, { phase: PHASE_ACTION, my_side: 0, charge_state: 0 }, "rifle")).toBe(
      "Press F to pick up the rifle",
    );
  });

  it("picks up what the engine would: the charge first for attackers, else the nearest weapon in reach", () => {
    const me = { x: 1000, y: 1000 };
    const gun = { kind: DROP_WEAPON, x: 1300, y: 1000, item: 1 };
    const near = { kind: DROP_WEAPON, x: 1100, y: 1000, item: 2 };
    const charge = { kind: DROP_CHARGE, x: 1500, y: 1000, item: 0 };
    expect(pickupTarget(me, [gun, near, charge], 900, true)).toBe(charge);
    expect(pickupTarget(me, [gun, near, charge], 900, false)).toBe(near);
    expect(pickupTarget(me, [charge], 900, false)).toBeNull();
    expect(pickupTarget(me, [gun], 200, true)).toBeNull();
  });

  it("converts tiles to fixed-point world units", () => {
    expect(toWorld(1.5, 2)).toEqual([1536, 2048]);
    expect(toWorld(0.0004, 0)).toEqual([0, 0]);
  });
});
