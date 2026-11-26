// Live validation. The verdict on whether a match can be played on the map comes from the engine itself
// (`Map::parse` compiled to WebAssembly, `ibc_map_check`), so the editor never accepts a map that
// `ibc run --map` rejects. Two things the engine accepts without complaint are pointed out as well, from its
// own summary: an open border and painted sites it drops. The full rule set is `ibc-maptool lint`, behind
// the Lint button when the page is served by `ibc view --editor`.

import { docHeight, docWidth, type MapDoc } from "./doc";

export interface SiteSummary {
  id: number;
  min_tx: number;
  min_ty: number;
  max_tx: number;
  max_ty: number;
  centre_x: number;
  centre_y: number;
}

/** What the engine makes of the map (`ibc_engine_wasm::map_summary`). */
export interface MapSummary {
  name: string;
  width: number;
  height: number;
  team_size: number;
  sites: SiteSummary[];
  spawns: [number, number];
  hash: string;
}

export interface Finding {
  severity: "error" | "warning";
  code: string;
  message: string;
}

export interface CheckResult {
  summary: MapSummary | null;
  findings: Finding[];
}

interface MapExports {
  memory: WebAssembly.Memory;
  ibc_alloc(len: number): number;
  ibc_free(ptr: number, len: number): void;
  ibc_map_check?(ptr: number, len: number): number;
  ibc_map_ptr?(): number;
  ibc_map_len?(): number;
  ibc_version_ptr(): number;
  ibc_version_len(): number;
}

/** `Map::parse` in the engine: the summary JSON, or the engine's error. */
export type EngineParse = (text: string) => { ok: true; summary: MapSummary } | { ok: false; error: string };

const utf8 = new TextEncoder();
const fromUtf8 = new TextDecoder();

/** Wrap an engine module that has the map export, or return null for a build that predates it. */
export function engineParse(instance: WebAssembly.Instance): { version: string; parse: EngineParse } | null {
  const x = instance.exports as unknown as MapExports;
  if (!x.ibc_map_check || !x.ibc_map_ptr || !x.ibc_map_len) return null;
  const str = (ptr: number, len: number) => (len ? fromUtf8.decode(new Uint8Array(x.memory.buffer, ptr, len)) : "");
  const parse: EngineParse = (text) => {
    const bytes = utf8.encode(text);
    const ptr = x.ibc_alloc(bytes.length);
    new Uint8Array(x.memory.buffer, ptr, bytes.length).set(bytes);
    const rc = x.ibc_map_check!(ptr, bytes.length);
    x.ibc_free(ptr, bytes.length);
    const out = str(x.ibc_map_ptr!(), x.ibc_map_len!());
    return rc === 0 ? { ok: true, summary: JSON.parse(out) as MapSummary } : { ok: false, error: out };
  };
  return { version: str(x.ibc_version_ptr(), x.ibc_version_len()), parse };
}

/** Order engine versions by their numeric parts (`0.10.0` after `0.9.0`), not as text. */
export function compareVersions(a: string, b: string): number {
  const pa = a.split(/[.+-]/);
  const pb = b.split(/[.+-]/);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const x = pa[i] ?? "";
    const y = pb[i] ?? "";
    const d = /^\d+$/.test(x) && /^\d+$/.test(y) ? Number(x) - Number(y) : x.localeCompare(y);
    if (d !== 0) return d;
  }
  return 0;
}

/** The newest engine build the page ships (`engine/index.json`) that can check maps. */
export async function loadEngineParse(base: string = document.baseURI): Promise<{ version: string; parse: EngineParse }> {
  const index = (await (await fetch(new URL("engine/index.json", base))).json()) as { versions?: unknown };
  const versions = Array.isArray(index.versions) ? index.versions.map(String) : [];
  for (const v of versions.sort(compareVersions).reverse()) {
    if (!/^[0-9A-Za-z][0-9A-Za-z.+-]*$/.test(v)) continue;
    const res = await fetch(new URL(`engine/${v}.wasm`, base));
    if (!res.ok) continue;
    const { instance } = await WebAssembly.instantiate(await res.arrayBuffer(), {});
    const wrapped = engineParse(instance);
    if (wrapped) return wrapped;
  }
  throw new Error("no engine build with the map check is shipped (rebuild the engine and run `npm run engine`)");
}

/** Validate `doc` with the engine, plus the two notes the engine's summary makes possible. */
export function check(doc: MapDoc, parse: EngineParse, text: string): CheckResult {
  const findings: Finding[] = [];
  const w = docWidth(doc);
  const h = docHeight(doc);
  const open: string[] = [];
  doc.rows.forEach((row, y) => {
    for (let x = 0; x < w; x++) {
      if ((x === 0 || y === 0 || x === w - 1 || y === h - 1) && row[x] !== "#") open.push(`(${x},${y})`);
    }
  });
  if (open.length) {
    findings.push({
      severity: "error",
      code: "open-border",
      message: `border tiles must be walls: ${open.slice(0, 4).join(" ")}${open.length > 4 ? ` and ${open.length - 4} more` : ""}`,
    });
  }
  const result = parse(text);
  if (!result.ok) {
    findings.unshift({ severity: "error", code: "engine-parse", message: `the engine rejects this map: ${result.error}` });
    return { summary: null, findings };
  }
  const painted = new Set<string>();
  for (const row of doc.rows) for (const c of row) if (c >= "1" && c <= "9") painted.add(c);
  const dropped = [...painted].filter((c) => Number(c) > result.summary.sites.length).sort();
  if (dropped.length) {
    findings.push({
      severity: "error",
      code: "site-numbering",
      message: `the engine reads ${result.summary.sites.length} site(s) and ignores site ${dropped.join(", ")}: number sites from 1 without gaps`,
    });
  }
  return { summary: result.summary, findings };
}
