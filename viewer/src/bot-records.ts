// The private part of a replay: bot logs, per-turn fuel and debug drawings (`ibc_replay::records_json`).
// A replay only has these for the teams it was not redacted for (your own team, on the platform). Records of
// tick t belong to frame t: what the bots did while looking at that state.

import type { DebugDraw } from "./types";
import type { Perspective } from "./perspective";
import { el } from "./ui";

export interface BotRecords {
  redacted: [boolean, boolean];
  /** Debug drawings by tick. */
  draws: Map<number, DebugDraw[]>;
  /** `[tick, text]` per unit, in tick order. */
  logs: Map<number, [number, string][]>;
  /** `[tick, used, outOfFuel]` per unit, in tick order. */
  fuel: Map<number, [number, number, boolean][]>;
}

export const NO_RECORDS: BotRecords = { redacted: [true, true], draws: new Map(), logs: new Map(), fuel: new Map() };

function push<K, V>(map: Map<K, V[]>, key: K, value: V): void {
  const list = map.get(key);
  if (list) list.push(value);
  else map.set(key, [value]);
}

const isNum = (v: unknown): v is number => typeof v === "number";

/** Parse the engine's records JSON. Malformed entries are skipped; unparseable text gives no records. */
export function parseRecords(text: string | null): BotRecords {
  if (!text) return NO_RECORDS;
  let raw: Record<string, unknown>;
  try {
    raw = JSON.parse(text);
  } catch {
    return NO_RECORDS;
  }
  const rows = (key: string) => (Array.isArray(raw[key]) ? (raw[key] as unknown[]) : []).filter(Array.isArray);
  const red = Array.isArray(raw.redacted) ? raw.redacted : [true, true];
  const out: BotRecords = { redacted: [red[0] === true, red[1] === true], draws: new Map(), logs: new Map(), fuel: new Map() };
  for (const r of rows("draws") as unknown[][]) {
    if (r.length >= 8 && r.every(isNum)) push(out.draws, r[0] as number, r.slice(1, 8) as DebugDraw);
  }
  for (const r of rows("logs") as unknown[][]) {
    if (isNum(r[0]) && isNum(r[1]) && typeof r[2] === "string") push(out.logs, r[1], [r[0], r[2]]);
  }
  for (const r of rows("fuel") as unknown[][]) {
    if (isNum(r[0]) && isNum(r[1]) && isNum(r[2])) push(out.fuel, r[1], [r[0], r[2], r[3] === true]);
  }
  return out;
}

/** Entries of a tick-sorted list up to and including `tick`, at most `max` of them. */
export function upTo<T extends [number, ...unknown[]]>(list: T[] | undefined, tick: number, max: number): T[] {
  if (!list) return [];
  let end = 0;
  while (end < list.length && list[end][0] <= tick) end++;
  return list.slice(Math.max(0, end - max), end);
}

const LOG_LINES = 40;
const FUEL_TURNS = 24;

/** Placeholder panel: the selected unit's log up to the current tick and its fuel use per recent turn. */
export class BotRecordsPanel {
  readonly root = el("div", "bot-records");
  private readonly title = el("div", "section-title", "Bot log and fuel");
  private readonly note = el("div", "meta");
  private readonly fuel = el("div", "fuel-bars");
  private readonly log = el("pre", "bot-log");

  constructor(
    private readonly records: BotRecords,
    private readonly teamSize: number,
  ) {
    this.root.append(this.title, this.note, this.fuel, this.log);
  }

  update(tick: number, perspective: Perspective): void {
    if (perspective.kind !== "unit") {
      this.show("Click a unit to see its log and fuel.");
      return;
    }
    const unit = perspective.id;
    const team = unit >= this.teamSize ? 1 : 0;
    if (this.records.redacted[team]) {
      this.show(`Team ${"AB"[team]}'s logs and fuel are not in this replay.`);
      return;
    }
    const turns = upTo(this.records.fuel.get(unit), tick, FUEL_TURNS);
    const last = turns[turns.length - 1];
    this.note.textContent = last
      ? `unit ${unit} · tick ${last[0]}: ${last[1]} fuel${last[2] ? " (ran out)" : ""}`
      : `unit ${unit} · no turns yet`;
    // Bars are scaled to the busiest turn shown.
    const top = Math.max(1, ...turns.map((t) => t[1]));
    this.fuel.replaceChildren(
      ...turns.map(([t, used, out]) => {
        const bar = el("i", out ? "out" : "");
        bar.style.height = `${Math.min(100, (used / top) * 100)}%`;
        bar.title = `tick ${t}: ${used} fuel${out ? ", ran out" : ""}`;
        return bar;
      }),
    );
    this.log.textContent = upTo(this.records.logs.get(unit), tick, LOG_LINES)
      .map(([t, text]) => `${t}: ${text.replace(/\n$/, "")}`)
      .join("\n");
  }

  private show(message: string): void {
    this.note.textContent = message;
    this.fuel.replaceChildren();
    this.log.textContent = "";
  }
}
