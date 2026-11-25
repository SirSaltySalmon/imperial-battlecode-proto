// The engine module (`ibc_engine_wasm.wasm`, no imports) and the frames it produces.
//
// One build per engine version lives at `engine/<engine_version>.wasm` next to the page; `engine/index.json`
// lists what is shipped. A replay is always opened with the build its header names, because loading
// re-simulates the match and checks every state hash.

import { parseHeaderJson } from "./ibcr";
import type { Frame, Header } from "./types";
import type { ViewRules } from "./vision";

/** The numeric fields of the engine's rules export. */
const RULE_NUMBERS = [
  "unit_radius",
  "fov_half",
  "vision_range",
  "sense_radius",
  "max_hp",
  "plant_ticks",
  "defuse_ticks",
  "fuse_ticks",
] as const;

interface EngineExports {
  memory: WebAssembly.Memory;
  ibc_alloc(len: number): number;
  ibc_free(ptr: number, len: number): void;
  ibc_load(ptr: number, len: number): number;
  ibc_frame_count(): number;
  ibc_frame_ptr(i: number): number;
  ibc_frame_len(i: number): number;
  ibc_header_ptr(): number;
  ibc_header_len(): number;
  ibc_error_ptr(): number;
  ibc_error_len(): number;
  ibc_version_ptr(): number;
  ibc_version_len(): number;
  /** Absent in engine builds older than the rules export. */
  ibc_rules_ptr?(): number;
  ibc_rules_len?(): number;
}

/** Where frame JSON comes from: the engine's memory in the app, plain strings in tests. */
export interface FrameSource {
  readonly count: number;
  text(i: number): string;
}

const utf8 = new TextDecoder();

export class Engine {
  private readonly x: EngineExports;

  constructor(instance: WebAssembly.Instance) {
    this.x = instance.exports as unknown as EngineExports;
  }

  static async instantiate(bytes: BufferSource): Promise<Engine> {
    const { instance } = await WebAssembly.instantiate(bytes, {});
    return new Engine(instance);
  }

  private str(ptr: number, len: number): string {
    // The buffer is looked up on every read: it is replaced whenever the module's memory grows.
    return len === 0 ? "" : utf8.decode(new Uint8Array(this.x.memory.buffer, ptr, len));
  }

  version(): string {
    return this.str(this.x.ibc_version_ptr(), this.x.ibc_version_len());
  }

  /**
   * The engine's whole rules export as parsed JSON (the buy menu reads its tables), or `null` for a build
   * that predates the export.
   */
  rawRules(): unknown {
    if (!this.x.ibc_rules_ptr || !this.x.ibc_rules_len) return null;
    try {
      return JSON.parse(this.str(this.x.ibc_rules_ptr(), this.x.ibc_rules_len()));
    } catch {
      return null;
    }
  }

  /**
   * The rules this engine build simulates with (sizes, timings, weapon names by id), or `null` for a build
   * that predates the export. Only fields of the right type are kept.
   */
  rules(): Partial<ViewRules> | null {
    const raw = this.rawRules();
    if (typeof raw !== "object" || raw === null) return null;
    const fields = raw as Record<string, unknown>;
    const out: Partial<ViewRules> = {};
    for (const key of RULE_NUMBERS) {
      if (typeof fields[key] === "number") out[key] = fields[key];
    }
    const names = fields.weapons;
    if (Array.isArray(names) && names.every((n) => typeof n === "string")) out.weapons = names as string[];
    return out;
  }

  /**
   * Re-simulate a replay. The returned frames stay inside the module's memory (one JSON string per tick)
   * and are decoded on demand; they are valid until the next `load` on this engine.
   */
  load(bytes: Uint8Array): { header: Header; frames: FrameSource } {
    const ptr = this.x.ibc_alloc(bytes.length);
    new Uint8Array(this.x.memory.buffer, ptr, bytes.length).set(bytes);
    const rc = this.x.ibc_load(ptr, bytes.length);
    this.x.ibc_free(ptr, bytes.length);
    if (rc !== 0) {
      throw new Error(this.str(this.x.ibc_error_ptr(), this.x.ibc_error_len()) || "the engine rejected the replay");
    }
    const header = parseHeaderJson(this.str(this.x.ibc_header_ptr(), this.x.ibc_header_len()));
    const count = this.x.ibc_frame_count();
    const text = (i: number) => this.str(this.x.ibc_frame_ptr(i), this.x.ibc_frame_len(i));
    return { header, frames: { count, text } };
  }
}

/** Engine versions become file names, so only plain version characters are let through. */
export function engineFile(version: string): string {
  if (!/^[0-9A-Za-z][0-9A-Za-z.+-]*$/.test(version)) {
    throw new Error(`replay names an invalid engine version ${JSON.stringify(version)}`);
  }
  return `engine/${version}.wasm`;
}

const engines = new Map<string, Promise<Engine>>();

/** Fetch and instantiate the engine build for `version` (cached for the page's lifetime). */
export function fetchEngine(version: string, base: string = document.baseURI): Promise<Engine> {
  let p = engines.get(version);
  if (!p) {
    p = fetchEngineUncached(version, base);
    engines.set(version, p);
    p.catch(() => engines.delete(version));
  }
  return p;
}

async function fetchEngineUncached(version: string, base: string): Promise<Engine> {
  const url = new URL(engineFile(version), base);
  const res = await fetch(url).catch(() => null);
  const type = res?.headers.get("content-type") ?? "";
  // Static hosts with an SPA fallback answer a missing file with index.html and status 200.
  if (!res || !res.ok || type.includes("text/html")) {
    const shipped = await shippedVersions(base);
    throw new Error(
      `this viewer has no engine build for version ${version}` +
        (shipped.length ? ` (it ships ${shipped.join(", ")})` : " (it ships none: run `npm run engine`)"),
    );
  }
  // Not `instantiateStreaming`: that needs `application/wasm`, which not every static host sends.
  return Engine.instantiate(await res.arrayBuffer());
}

async function shippedVersions(base: string): Promise<string[]> {
  try {
    const res = await fetch(new URL("engine/index.json", base));
    const json = (await res.json()) as { versions?: unknown };
    return Array.isArray(json.versions) ? json.versions.map(String) : [];
  } catch {
    return [];
  }
}

/**
 * Frames parsed on demand with a small most-recently-used cache: playback touches two neighbouring frames
 * at a time, so a long match never holds more than `capacity` parsed frames.
 */
export class FrameStore {
  private readonly cache = new Map<number, Frame>();

  constructor(
    private readonly source: FrameSource,
    private readonly capacity = 48,
  ) {}

  get count(): number {
    return this.source.count;
  }

  /** Parse frame `i` without caching it (for one-off scans). */
  parse(i: number): Frame {
    if (i < 0 || i >= this.source.count) throw new RangeError(`frame ${i} out of range`);
    return JSON.parse(this.source.text(i)) as Frame;
  }

  get(i: number): Frame {
    const hit = this.cache.get(i);
    if (hit) {
      this.cache.delete(i);
      this.cache.set(i, hit);
      return hit;
    }
    const frame = this.parse(i);
    this.cache.set(i, frame);
    if (this.cache.size > this.capacity) {
      this.cache.delete(this.cache.keys().next().value as number);
    }
    return frame;
  }

  get cached(): number {
    return this.cache.size;
  }
}
