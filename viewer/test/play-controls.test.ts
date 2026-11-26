import { describe, expect, it } from "vitest";
import { heldInput, heldKey, keyAction, moveVector, NOTHING_HELD, sameHeld, toWorld } from "../src/play/controls";
import { PHASE_ACTION, PHASE_BUY, PHASE_DRAFT, PHASE_PLANTED, PHASE_ROUND_END } from "../src/types";

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
    expect(a).toEqual({ move: [0, 1], walk: true, aim: [100, 200], fire: true, interact: false });
    expect(sameHeld(a, heldInput({ ...NOTHING_HELD, down: true, walk: true, fire: true }, [100, 200]))).toBe(true);
    expect(sameHeld(a, heldInput({ ...NOTHING_HELD, down: true, walk: true, fire: true }, [101, 200]))).toBe(false);
    expect(heldInput(NOTHING_HELD, null).aim).toBeNull();
  });

  it("gives digits the meaning of the phase", () => {
    expect(keyAction("Digit1", PHASE_DRAFT)).toEqual({ act: "draft", kit: 0 });
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
    expect(keyAction("KeyG", PHASE_ACTION)).toEqual({ act: "pickup" });
    expect(keyAction("KeyC", PHASE_ACTION)).toEqual({ act: "cancel" });
    expect(keyAction("KeyR", PHASE_ROUND_END)).toBeNull();
  });

  it("converts tiles to fixed-point world units", () => {
    expect(toWorld(1.5, 2)).toEqual([1536, 2048]);
    expect(toWorld(0.0004, 0)).toEqual([0, 0]);
  });
});
