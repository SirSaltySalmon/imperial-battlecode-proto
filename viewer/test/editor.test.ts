import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { validMapName } from "../src/editor/api";
import { check, compareVersions, type EngineParse, type MapSummary } from "../src/editor/check";
import { cloneDoc, getHeader, parseDoc, resizeDoc, scaffold, serializeDoc, setHeader, type MapDoc } from "../src/editor/doc";
import { History } from "../src/editor/history";
import { floodFill, lineTiles, paintBrush, paintRect, paintTile } from "../src/editor/paint";
import { applyOp, mirrorHalf, partners, type SymmetryOptions } from "../src/editor/transform";
import { parseMap } from "../src/map";

const MAPS = join(__dirname, "../../maps");
const NO_SYM: SymmetryOptions = { symmetry: "none", swapSides: false, pairSites: true };
const doc = (rows: string[]): MapDoc => ({ header: ["name: t"], rows });

describe("map text", () => {
  const files = readdirSync(MAPS).filter((f) => f.endsWith(".ibcmap"));

  it("round-trips every pool map byte for byte", () => {
    expect(files.length).toBeGreaterThan(10);
    for (const f of files) {
      const text = readFileSync(join(MAPS, f), "utf8");
      expect(serializeDoc(parseDoc(text)), f).toBe(text);
    }
  });

  it("keeps notes and unknown keys, and edits header values", () => {
    const text = "name: t\n// a note\nauthor: me\nteam_size: 3\n---\n###\n#a#\n###\n";
    const d = parseDoc(text);
    expect(serializeDoc(d)).toBe(text);
    setHeader(d, "name", "u");
    setHeader(d, "seed", "4");
    expect(getHeader(d, "name")).toBe("u");
    expect(d.header).toEqual(["name: u", "// a note", "author: me", "team_size: 3", "seed: 4"]);
  });

  it("rejects what the editor cannot paint on", () => {
    expect(() => parseDoc("name: t\n###\n")).toThrow(/---/);
    expect(() => parseDoc("name: t\n---\n###\n##\n")).toThrow(/wide/);
    expect(() => parseDoc("name: t\n---\n#x#\n")).toThrow(/unknown tile/);
  });

  it("accepts CRLF files and writes LF", () => {
    expect(serializeDoc(parseDoc("name: t\r\n---\r\n###\r\n"))).toBe("name: t\n---\n###\n");
  });
});

describe("painting and undo", () => {
  it("paints, undoes and redoes back to the original text", () => {
    const text = readFileSync(join(MAPS, "crossfire.ibcmap"), "utf8");
    let d = parseDoc(text);
    const history = new History();
    const before = d;
    const work = cloneDoc(d);
    expect(paintBrush(work, 5, 5, 3, "#", NO_SYM)).toBeGreaterThan(0);
    history.commit(before, work);
    d = work;
    expect(serializeDoc(d)).not.toBe(text);
    const undone = history.undo(d)!;
    expect(serializeDoc(undone)).toBe(text);
    const redone = history.redo(undone)!;
    expect(serializeDoc(redone)).toBe(serializeDoc(work));
    expect(history.canRedo).toBe(false);
  });

  it("does not record an edit that changed nothing", () => {
    const h = new History();
    const d = doc(["###"]);
    h.commit(d, cloneDoc(d));
    expect(h.canUndo).toBe(false);
  });

  it("paints the mirror image with sides and sites paired", () => {
    const d = doc(["......", "......"]);
    const sym: SymmetryOptions = { symmetry: "h", swapSides: true, pairSites: true };
    paintTile(d, 0, 0, "a", sym);
    paintTile(d, 1, 1, "1", sym);
    expect(d.rows).toEqual(["a....d", ".1..2."]);
    expect(partners(6, 2, 0, 0, "#", { ...sym, symmetry: "point" })).toEqual([
      [0, 0, "#"],
      [5, 1, "#"],
    ]);
    expect(partners(5, 1, 2, 0, "3", { ...sym, symmetry: "h" })).toEqual([[2, 0, "3"]]);
  });

  it("fills rectangles, lines and regions", () => {
    const d = doc(["#####", "#...#", "#.#.#", "#####"]);
    paintRect(d, 1, 1, 2, 1, "o", NO_SYM);
    expect(d.rows[1]).toBe("#oo.#");
    expect(floodFill(d, 3, 1, "1", NO_SYM)).toBe(2);
    expect(d.rows).toEqual(["#####", "#oo1#", "#.#1#", "#####"]);
    expect(lineTiles(0, 0, 3, 1)).toEqual([
      [0, 0],
      [1, 0],
      [2, 1],
      [3, 1],
    ]);
  });
});

describe("whole-map edits match ibc-maptool", () => {
  // The same grids as the tests in crates/maptool/src/transform.rs.
  const m = doc(["#a.", "..1", "d.o", "###"]);

  it("rotates and flips", () => {
    expect(applyOp(m, "rot90").rows).toEqual(["#d.#", "#..a", "#o1."]);
    expect(applyOp(applyOp(m, "rot90"), "rot90")).toEqual(applyOp(m, "rot180"));
    expect(applyOp(applyOp(m, "rot90"), "rot270")).toEqual(m);
    expect(applyOp(m, "flip-h").rows[0]).toBe(".a#");
    expect(applyOp(m, "flip-v").rows[0]).toBe("###");
    expect(applyOp(m, "swap-sides").rows).toEqual(["#d.", "..1", "a.o", "###"]);
    expect(applyOp(applyOp(m, "transpose"), "transpose")).toEqual(m);
  });

  it("mirrors a half and renumbers its sites", () => {
    expect(mirrorHalf(doc(["#a1..", "#.o..", "#...."]), "h", true).rows).toEqual(["#a1d#", "#.o.#", "#...#"]);
    const half = doc(["a1.", "...", "...", "..."]);
    expect(mirrorHalf(half, "point", true).rows).toEqual(["a1.", "...", "...", ".2d"]);
    expect(mirrorHalf(half, "v", false).rows).toEqual(["a1.", "...", "...", "a2."]);
    expect(mirrorHalf(doc(["a1.", ".2.", "..."]), "v", false).rows).toEqual(["a1.", ".2.", "a3."]);
  });

  it("resizes inside a wall border", () => {
    const r = resizeDoc(doc(["#####", "#a.d#", "#####"]), 6, 4);
    expect(r.rows).toEqual(["######", "#a.d.#", "#....#", "######"]);
    expect(() => resizeDoc(m, 2, 9)).toThrow();
    expect(() => resizeDoc(m, 20.5, 9)).toThrow(/whole tiles/);
  });

  it("builds the same scaffold as ibc-maptool new", () => {
    const s = scaffold(16, 12, 3, "fresh");
    expect(s.header).toEqual(["name: fresh", "team_size: 3"]);
    expect(s.rows[6]).toBe("#aa.....#....dd#");
    expect(s.rows[2]).toBe("#.......111....#");
    const map = parseMap(serializeDoc(s));
    expect(map.spawns[0].length).toBe(6);
    expect(map.spawns[1].length).toBe(6);
    expect(map.sites.map((x) => x.tiles.length)).toEqual([9, 9]);
    expect(s.rows.every((r) => r.length === 16)).toBe(true);
  });
});

describe("validation", () => {
  const summary = (sites: number): MapSummary => ({
    name: "t",
    width: 5,
    height: 3,
    team_size: 1,
    sites: Array.from({ length: sites }, (_, id) => ({ id, min_tx: 0, min_ty: 0, max_tx: 1, max_ty: 1, centre_x: 0, centre_y: 0 })),
    spawns: [1, 1],
    hash: "0",
  });

  it("reports the engine's error, an open border and dropped sites", () => {
    const reject: EngineParse = () => ({ ok: false, error: "map has no plant site" });
    const bad = check(doc(["#####", "#a.d.", "#####"]), reject, "");
    expect(bad.summary).toBeNull();
    expect(bad.findings.map((f) => f.code)).toEqual(["engine-parse", "open-border"]);
    const accept: EngineParse = () => ({ ok: true, summary: summary(1) });
    const gap = check(doc(["#####", "#a13#", "#d..#", "#####"]), accept, "");
    expect(gap.findings.map((f) => f.code)).toEqual(["site-numbering"]);
    expect(gap.findings[0].message).toMatch(/ignores site 3/);
    expect(check(doc(["####", "#a1#", "#d.#", "####"]), accept, "").findings).toEqual([]);
  });

  it("picks the newest engine by version number, not by text", () => {
    expect(["0.10.0", "0.9.0", "0.9.1", "1.0.0"].sort(compareVersions)).toEqual(["0.9.0", "0.9.1", "0.10.0", "1.0.0"]);
  });

  it("accepts only the names the server accepts", () => {
    for (const ok of ["custom1", "my-map_2"]) expect(validMapName(ok)).toBe(true);
    for (const bad of ["", "../x", "a b", "x.ibcmap", "con", "COM1", "x".repeat(65)]) expect(validMapName(bad)).toBe(false);
  });
});
