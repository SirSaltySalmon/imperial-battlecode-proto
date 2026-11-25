import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { Engine, engineFile, FrameStore, type FrameSource } from "../src/engine";
import { parseContainer, parseHeaderJson, readHeader } from "../src/ibcr";
import { parseMap, segmentWall, tileAt } from "../src/map";
import { buildTimeline, recentEvents, roundIndexAt } from "../src/timeline";
import { EVENT_KILL, EVENT_PLANT, TILE_LOW, TILE_WALL } from "../src/types";
import { container, frame, headerJson, MAP_TEXT } from "./helpers";

describe(".ibcr container", () => {
  it("rejects a bad magic and an unknown format version", () => {
    expect(() => parseContainer(new Uint8Array([1, 2, 3]))).toThrow(/bad magic/);
    expect(() => parseContainer(new TextEncoder().encode("NOPE0000rest"))).toThrow(/bad magic/);
    expect(() => parseContainer(container(headerJson(), undefined, 2))).toThrow(/format version 2/);
  });

  it("reads the header without needing the body", async () => {
    const body = new Uint8Array(300_000).map((_, i) => (i * 31) & 0xff);
    const h = await readHeader(container(headerJson(), body));
    expect(h.engine_version).toBe("0.1.0");
    expect(h.bots.map((b) => b.name)).toEqual(["alpha", "beta"]);
    expect(h.map_text).toBe(MAP_TEXT);
  });

  it("reads a header from a view into a larger buffer", async () => {
    const c = container(headerJson());
    const padded = new Uint8Array(c.length + 10);
    padded.set(c, 5);
    expect((await readHeader(padded.subarray(5, 5 + c.length))).engine_version).toBe("0.1.0");
  });

  it("reports a corrupt stream and a missing header line", async () => {
    const c = container(headerJson());
    await expect(readHeader(c.subarray(0, 12))).rejects.toThrow(/corrupt/);
    const junk = new Uint8Array(40).fill(0xff);
    junk.set(c.subarray(0, 8));
    await expect(readHeader(junk)).rejects.toThrow(/corrupt/);
  });

  it("keeps a u64 seed exact", () => {
    const text = headerJson().replace('"seed":1', '"seed":18446744073709551615');
    expect(parseHeaderJson(text).seed).toBe("18446744073709551615");
  });

  it("is not fooled by `seed` inside a string", () => {
    const text = headerJson({ map_text: 'x,"seed":7', seed: 42 });
    expect(parseHeaderJson(text).seed).toBe("42");
  });

  it("rejects header JSON that is not a header", () => {
    expect(() => parseHeaderJson("{")).toThrow(/header is invalid/);
    expect(() => parseHeaderJson("{}")).toThrow(/engine_version/);
  });

  it("only turns plain versions into engine file names", () => {
    expect(engineFile("0.1.0")).toBe("engine/0.1.0.wasm");
    expect(engineFile("1.2.3-rc.1+build5")).toBe("engine/1.2.3-rc.1+build5.wasm");
    for (const bad of ["../x", "a/b", "", "0.1.0?x", ".hidden"]) expect(() => engineFile(bad)).toThrow();
  });
});

describe("frame JSON", () => {
  const source = (frames: object[]): FrameSource & { reads: number[] } => {
    const reads: number[] = [];
    return {
      count: frames.length,
      reads,
      text(i) {
        reads.push(i);
        return JSON.stringify(frames[i]);
      },
    };
  };

  it("parses lazily and keeps a bounded cache", () => {
    const src = source(Array.from({ length: 10 }, (_, i) => frame({ tick: i })));
    const store = new FrameStore(src, 3);
    expect(src.reads).toEqual([]);
    expect(store.get(4).tick).toBe(4);
    expect(store.get(4)).toBe(store.get(4));
    expect(src.reads).toEqual([4]);
    store.get(5);
    store.get(6);
    store.get(4); // refresh 4, so 5 is now the oldest
    store.get(7);
    expect(store.cached).toBe(3);
    src.reads.length = 0;
    store.get(4);
    expect(src.reads).toEqual([]);
    store.get(5);
    expect(src.reads).toEqual([5]);
    expect(() => store.get(10)).toThrow(RangeError);
  });

  it("indexes rounds, round results and public events", () => {
    const frames = [
      frame({ tick: 0, round: 0, phase: 0 }),
      frame({ tick: 1, round: 1 }),
      frame({ tick: 2, round: 1, events: [{ kind: 6, actor: 0, target: 3, x: 0, y: 0, value: 20 }] }),
      frame({ tick: 3, round: 1, events: [{ kind: EVENT_PLANT, actor: 0, target: -1, x: 5, y: 5, value: 0 }] }),
      frame({
        tick: 4,
        round: 1,
        events: [
          { kind: EVENT_KILL, actor: 0, target: 3, x: 0, y: 0, value: 1 },
          { kind: 5, actor: 0, target: -1, x: 0, y: 0, value: 2 },
        ],
      }),
      frame({ tick: 5, round: 2, attackers: 1 }),
    ];
    const tl = buildTimeline(new FrameStore(source(frames)));
    expect(tl.rounds.map((r) => [r.round, r.start, r.end])).toEqual([
      [0, 0, 0],
      [1, 1, 4],
      [2, 5, 5],
    ]);
    expect(tl.rounds[1]).toMatchObject({ winner: 0, reason: 2, plant: 3, attackers: 0 });
    expect(tl.rounds[2]).toMatchObject({ winner: null, attackers: 1, plant: -1 });
    expect(tl.events.map((e) => e.kind)).toEqual([EVENT_PLANT, EVENT_KILL, 5]);
    expect([0, 1, 4, 5].map((f) => roundIndexAt(tl, f))).toEqual([0, 1, 1, 2]);
    expect(recentEvents(tl, 4, 10, [EVENT_KILL]).map((e) => e.frame)).toEqual([4]);
    expect(recentEvents(tl, 3, 10, [EVENT_KILL])).toEqual([]);
    expect(recentEvents(tl, 5, 1, [EVENT_KILL])).toEqual([]);
    expect(recentEvents(tl, 5, 2, [EVENT_KILL, EVENT_PLANT]).map((e) => e.kind)).toEqual([EVENT_KILL]);
  });
});

describe("map", () => {
  const map = parseMap(MAP_TEXT);

  it("parses tiles, spawns and sites", () => {
    expect([map.name, map.teamSize, map.width, map.height]).toEqual(["test", 2, 10, 6]);
    expect(tileAt(map, 0, 0)).toBe(TILE_WALL);
    expect(tileAt(map, 6, 3)).toBe(TILE_LOW);
    expect(tileAt(map, -1, 2)).toBe(TILE_WALL);
    expect(map.spawns[0]).toEqual([[1, 1], [2, 1]]);
    expect(map.spawns[1]).toEqual([[8, 3], [8, 4]]);
    expect(map.sites).toHaveLength(1);
    expect(map.sites[0]).toMatchObject({ id: 0, minTx: 7, minTy: 1, maxTx: 8, maxTy: 3 });
  });

  it("accepts CRLF and rejects ragged rows", () => {
    expect(parseMap(MAP_TEXT.replace(/\n/g, "\r\n")).width).toBe(10);
    expect(() => parseMap("---\n###\n##\n")).toThrow(/width/);
    expect(() => parseMap("name: x\n")).toThrow(/no grid/);
  });

  it("finds the first wall on a segment", () => {
    // Row 2 has a wall at tile x = 4: a ray along it from x = 1.5 tiles stops at x = 4 tiles.
    expect(segmentWall(map, 1536, 2560, 8000, 2560)).toEqual([4096, 2560]);
    // Row 1 is open from x = 1 to x = 8.
    expect(segmentWall(map, 1536, 1536, 8000, 1536)).toBeNull();
    // Low cover does not block.
    expect(segmentWall(map, 5632, 3584, 8000, 3584)).toBeNull();
    // Diagonal into the top wall.
    const hit = segmentWall(map, 2560, 2560, 2560 + 3000, 2560 - 3000)!;
    expect(hit[1]).toBeCloseTo(1024, 6);
  });
});

const wasm = fileURLToPath(new URL("../../target/wasm32-unknown-unknown/release/ibc_engine_wasm.wasm", import.meta.url));
const sample = fileURLToPath(new URL("../../fixtures/sample.ibcr", import.meta.url));

// Needs `cargo build -p ibc-engine-wasm --release --target wasm32-unknown-unknown`; skipped without it.
describe.skipIf(!existsSync(wasm) || !existsSync(sample))("engine module with fixtures/sample.ibcr", () => {
  it("loads the replay, agrees with the JS header reader, and serves frames", async () => {
    const bytes = new Uint8Array(readFileSync(sample));
    const peek = await readHeader(bytes);
    const engine = await Engine.instantiate(readFileSync(wasm));
    expect(engine.version()).toBe(peek.engine_version);
    const { header, frames } = engine.load(bytes);
    expect(header).toEqual(peek);
    expect(frames.count).toBe(header.result.ticks + 1);
    const store = new FrameStore(frames);
    const last = store.get(frames.count - 1);
    expect(last.score).toEqual(header.result.score);
    expect(last.tick).toBe(header.result.ticks);
    const tl = buildTimeline(store);
    expect(tl.rounds.filter((r) => r.winner !== null)).toHaveLength(header.result.score[0] + header.result.score[1]);
    expect(parseMap(header.map_text).width).toBeGreaterThan(0);
  });

  it("returns the engine's message for a replay it cannot run", async () => {
    const engine = await Engine.instantiate(readFileSync(wasm));
    expect(() => engine.load(new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9]))).toThrow(/bad magic/);
    expect(() => engine.load(container(headerJson({ engine_version: "9.9.9" })))).toThrow(/engine 9\.9\.9/);
  });
});
