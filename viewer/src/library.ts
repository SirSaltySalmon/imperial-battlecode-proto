// The replay folder `ibc view --replays DIR` offers: a menu in the header, a list on the empty page, and
// previous / next. A static host without the folder answers 404 and the page simply has no menu.

export interface LibraryEntry {
  /** Path below the folder, with `/` separators. */
  path: string;
  bots: [string, string];
  map: string;
  score: [number, number];
  winner: number | null;
  ticks: number;
  finished: boolean;
}

const LIST_URL = "replays/index.json";

/** The replays on offer, or none when the server has no folder. */
export async function fetchLibrary(): Promise<LibraryEntry[]> {
  try {
    const res = await fetch(new URL(LIST_URL, document.baseURI));
    if (!res.ok) return [];
    const list: unknown = await res.json();
    return Array.isArray(list) ? (list as LibraryEntry[]) : [];
  } catch {
    return [];
  }
}

/** What goes in `?replay=`: relative, so it works wherever the viewer is hosted. */
export function entryUrl(e: LibraryEntry): string {
  return "replays/" + e.path.split("/").map(encodeURIComponent).join("/");
}

/** "sapper 7-4 retaker · crossfire" */
export function entryLabel(e: LibraryEntry): string {
  const result = e.finished ? `${e.score[0]}-${e.score[1]}` : "unfinished";
  return `${e.bots[0]} ${result} ${e.bots[1]} · ${e.map}`;
}

/** The link that opens an entry in the viewer, keeping no other page parameter. */
export function entryHref(e: LibraryEntry): string {
  return "?replay=" + encodeURIComponent(entryUrl(e));
}

/** Index of the entry the page has open, given its `replay` parameter. */
export function currentIndex(list: LibraryEntry[], replayParam: string | null): number {
  if (!replayParam) return -1;
  const wanted = replayParam.replace(/^\.?\//, "");
  return list.findIndex((e) => entryUrl(e) === wanted || "replays/" + e.path === wanted);
}

/** The entry `delta` places from `index`, wrapping round; from "none open" the first or the last. */
export function neighbour(list: LibraryEntry[], index: number, delta: number): LibraryEntry | null {
  if (list.length === 0) return null;
  if (index < 0) return delta > 0 ? list[0] : list[list.length - 1];
  return list[(index + delta + list.length) % list.length];
}
