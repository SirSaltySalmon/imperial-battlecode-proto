// The map being edited, as text: header lines and a grid of tile characters. This is the twin of
// `ibc-maptool`'s `RawMap` (crates/maptool/src/raw.rs): `parseDoc` then `serializeDoc` gives back the file
// (blank lines and trailing spaces aside), so header notes and unknown keys survive an edit.

/** Tile characters of the `.ibcmap` grid (docs/maps.md). */
export const TILE_CHARS = "#.oad123456789";

export interface MapDoc {
  /** Header lines before `---`, verbatim (blank lines dropped). */
  header: string[];
  /** Grid rows, all the same width, every character in `TILE_CHARS`. */
  rows: string[];
}

export function isTileChar(c: string): boolean {
  return c.length === 1 && TILE_CHARS.includes(c);
}

/**
 * Read map text. Unlike the engine this is strict about the grid (rectangular, known tiles) because the
 * editor paints on a rectangle; the engine's own verdict on the rest comes from `engine-check.ts`.
 */
export function parseDoc(text: string): MapDoc {
  const lines = text.split(/\r?\n/);
  const header: string[] = [];
  let i = 0;
  let found = false;
  for (; i < lines.length; i++) {
    const line = lines[i].trimEnd();
    if (line === "---") {
      found = true;
      i++;
      break;
    }
    if (line !== "") header.push(line);
  }
  if (!found) throw new Error("missing `---` line between the header and the grid");
  const rows = lines
    .slice(i)
    .map((l) => l.trimEnd())
    .filter((l) => l !== "");
  if (rows.length === 0) throw new Error("map has no grid rows");
  const width = rows[0].length;
  rows.forEach((row, y) => {
    if (row.length !== width) throw new Error(`row ${y} is ${row.length} wide but row 0 is ${width}`);
    for (let x = 0; x < row.length; x++) {
      if (!isTileChar(row[x])) throw new Error(`unknown tile \`${row[x]}\` at (${x},${y})`);
    }
  });
  if (width > 128 || rows.length > 128) throw new Error("map larger than 128x128");
  return { header, rows };
}

/** The file text: LF line ends and a final newline, as the pool maps are stored. */
export function serializeDoc(doc: MapDoc): string {
  return [...doc.header, "---", ...doc.rows].join("\n") + "\n";
}

export function cloneDoc(doc: MapDoc): MapDoc {
  return { header: [...doc.header], rows: [...doc.rows] };
}

export function sameDoc(a: MapDoc, b: MapDoc): boolean {
  return (
    a.header.length === b.header.length &&
    a.rows.length === b.rows.length &&
    a.header.every((h, i) => h === b.header[i]) &&
    a.rows.every((r, i) => r === b.rows[i])
  );
}

export function docWidth(doc: MapDoc): number {
  return doc.rows[0]?.length ?? 0;
}

export function docHeight(doc: MapDoc): number {
  return doc.rows.length;
}

function headerKey(line: string): string | null {
  if (line.startsWith("//")) return null;
  const colon = line.indexOf(":");
  return colon < 0 ? null : line.slice(0, colon).trim();
}

/** Value of a `key: value` header line. */
export function getHeader(doc: MapDoc, key: string): string | null {
  for (const line of doc.header) {
    if (headerKey(line) === key) return line.slice(line.indexOf(":") + 1).trim();
  }
  return null;
}

/** Replace or append a `key: value` header line (in place). */
export function setHeader(doc: MapDoc, key: string, value: string): void {
  const line = `${key}: ${value}`;
  const at = doc.header.findIndex((l) => headerKey(l) === key);
  if (at >= 0) doc.header[at] = line;
  else doc.header.push(line);
}

export function tileOf(doc: MapDoc, x: number, y: number): string {
  return doc.rows[y]?.[x] ?? "#";
}

/** Set one tile (in place). Returns false if (x, y) is outside the grid or the tile is unchanged. */
export function setTile(doc: MapDoc, x: number, y: number, c: string): boolean {
  if (y < 0 || y >= doc.rows.length || x < 0 || x >= docWidth(doc)) return false;
  const row = doc.rows[y];
  if (row[x] === c) return false;
  doc.rows[y] = row.slice(0, x) + c + row.slice(x + 1);
  return true;
}

function wholeIn(n: number, lo: number, hi: number): boolean {
  return Number.isInteger(n) && n >= lo && n <= hi;
}

/**
 * Resize to `width` x `height`, keeping the top-left. The border moves with the size: the old right and bottom
 * border tiles become floor like every new tile, and the new border is wall.
 */
export function resizeDoc(doc: MapDoc, width: number, height: number): MapDoc {
  if (!wholeIn(width, 3, 128) || !wholeIn(height, 3, 128)) {
    throw new Error(`size ${width}x${height} is not whole tiles within 3x3 .. 128x128`);
  }
  const rows: string[] = [];
  for (let y = 0; y < height; y++) {
    let row = "";
    for (let x = 0; x < width; x++) {
      const border = x === 0 || y === 0 || x === width - 1 || y === height - 1;
      const kept = x < docWidth(doc) - 1 && y < docHeight(doc) - 1;
      row += border ? "#" : kept ? doc.rows[y][x] : ".";
    }
    rows.push(row);
  }
  return { header: [...doc.header], rows };
}

/**
 * A bordered, lint-clean starting point, the same grid as `ibc-maptool new` (`transform::scaffold`): spawns
 * at the left and right edges hidden from each other by a centre wall, and two sites on the defenders' side.
 */
export function scaffold(width: number, height: number, teamSize: number, name: string): MapDoc {
  if (!wholeIn(width, 16, 128) || !wholeIn(height, 12, 128)) {
    throw new Error(`size ${width}x${height} is not whole tiles within 16x12 .. 128x128`);
  }
  if (!wholeIn(teamSize, 1, 5)) throw new Error(`team size ${teamSize} is outside 1..5`);
  const g: string[][] = [];
  for (let y = 0; y < height; y++) {
    g.push([]);
    for (let x = 0; x < width; x++) {
      g[y].push(x === 0 || y === 0 || x === width - 1 || y === height - 1 ? "#" : ".");
    }
  }
  const mid = Math.floor(height / 2);
  for (let y = mid - 1; y <= mid + 1; y++) {
    for (let x = 0; x < 2; x++) {
      g[y][1 + x] = "a";
      g[y][width - 2 - x] = "d";
    }
  }
  for (let y = mid - 3; y < mid + 4; y++) g[y][Math.floor(width / 2)] = "#";
  for (const [digit, top] of [["1", 2], ["2", height - 5]] as const) {
    for (let y = top; y < top + 3; y++) {
      for (let x = width - 8; x < width - 5; x++) g[y][x] = digit;
    }
  }
  return { header: [`name: ${name}`, `team_size: ${teamSize}`], rows: g.map((r) => r.join("")) };
}
