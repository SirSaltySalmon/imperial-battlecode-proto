import { deflateRawSync } from "node:zlib";
import type { Frame, ProjectileFrame, UnitFrame } from "../src/types";

export const MAP_TEXT = [
  "name: test",
  "team_size: 2",
  "---",
  "##########",
  "#aa....1.#",
  "#...#..1.#",
  "#...#.o.d#",
  "#...#...d#",
  "##########",
  "",
].join("\n");

export function headerJson(overrides: Record<string, unknown> = {}): string {
  return JSON.stringify({
    engine_version: "0.1.0",
    ruleset_hash: "00d2cae8a4f4c61b",
    map_hash: "96f94801560ff9d6",
    map_text: MAP_TEXT,
    seed: 1,
    wasmtime_version: "",
    abi_version: 0,
    bots: [
      { name: "alpha", sha256: "aa", fuel_multiplier: 1 },
      { name: "beta", sha256: "bb", fuel_multiplier: 1 },
    ],
    result: { winner: 0, score: [1, 0], ticks: 3, finished: true },
    team_size: 1,
    redacted: [false, false],
    ...overrides,
  });
}

/** A `.ibcr` container around `header` and an arbitrary body. */
export function container(header: string, body: Uint8Array = new Uint8Array([0]), version = 2): Uint8Array {
  const payload = Buffer.concat([Buffer.from(header + "\n", "utf8"), body]);
  const out = new Uint8Array(8 + deflateRawSync(payload).length);
  out.set([0x49, 0x42, 0x43, 0x52]);
  new DataView(out.buffer).setUint32(4, version, true);
  out.set(deflateRawSync(payload), 8);
  return out;
}

export function unit(id: number, team: number, over: Partial<UnitFrame> = {}): UnitFrame {
  return {
    id,
    team,
    side: team,
    kit: 0,
    alive: true,
    state: 1,
    x: 2560,
    y: 2560,
    facing: 0,
    vx: 0,
    vy: 0,
    hp: 100,
    armor: 0,
    weapon: 0,
    ammo: 12,
    channel: 0,
    aim: 0,
    credits: 800,
    seen: "0",
    kills: 0,
    deaths: 0,
    sounds: [],
    ...over,
  };
}

export function frame(over: Partial<Frame> = {}): Frame {
  return {
    tick: 0,
    round: 1,
    phase: 2,
    phase_left: 100,
    score: [0, 0],
    attackers: 0,
    charge: { state: 0, x: 0, y: 0 },
    units: [],
    projectiles: [],
    effects: [],
    drops: [],
    events: [],
    debug: [],
    winner: null,
    ...over,
  };
}

export function bullet(over: Partial<ProjectileFrame> = {}): ProjectileFrame {
  return { owner: 0, team: 0, weapon: 0, kind: 0, x: 2000, y: 2000, vx: 1000, vy: 0, ...over };
}
