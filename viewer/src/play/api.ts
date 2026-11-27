// The endpoints of `ibc play` (crates/cli/src/play/server.rs) and the shapes they answer with.

import type { Frame, UnitFrame } from "../types";
import type { ViewRules } from "../vision";
import type { InputMsg } from "./controls";

export type Mode = "active" | "realtime" | "turn";

export interface Named {
  id: number;
  name: string;
}

export interface Setup {
  map_text: string;
  rules: Partial<ViewRules>;
  mode: Mode;
  /** Ticks per second of the clock; 0 in turn mode. */
  tps: number;
  team: number;
  team_size: number;
  /** Unit id of slot 0 of the player's team. */
  first_unit: number;
  bots: [string, string];
  core: {
    walk_speed: number;
    run_speed: number;
    turn_rate: number;
    buy_ticks: number;
    round_ticks: number;
    pickup_reach: number;
  };
  abilities: (Named & { price: number; max_charges: number; range: number })[];
  kits: (Named & { abilities: [number, number]; hp: number; speed_pct: number })[];
  items: (Named & { kind: number; price: number; value: number })[];
}

/** A unit row of the fogged frame. `hidden`: an enemy the team knows nothing about beyond alive or dead. */
export type PlayUnit = UnitFrame & { hidden?: boolean };

/** The player's unit as its bot reads it (`T_SELF`), plus its weapon slots. */
export interface Me {
  id: number;
  team: number;
  side: number;
  kit: number;
  state: number;
  x: number;
  y: number;
  facing: number;
  hp: number;
  armor: number;
  weapon: number;
  ammo: number;
  channel: number;
  credits: number;
  charges: [number, number];
  cooldown: [number, number];
  weapons: [number, number];
  slot: number;
}

/** `T_MATCH` as the player's bot reads it. */
export interface MatchView {
  tick: number;
  round: number;
  phase: number;
  phase_ticks_left: number;
  my_side: number;
  score_mine: number;
  score_theirs: number;
  allies_alive: number;
  enemies_alive: number;
  /** -1 while the team does not know where the charge is, else `CHARGE_*`. */
  charge_state: number;
  fuse_left: number;
  rounds_to_win: number;
}

export interface State {
  mode: Mode;
  /** The clock runs on its own (real time started, or time let run in active mode). */
  running: boolean;
  tick: number;
  /** The slot of the unit the player drives, or null while watching. */
  possessed: number | null;
  queued: number;
  last_acts: { act: string; a: number; code: number; result: string }[];
  done: { result?: string; replay?: string; outcome?: string; error?: string } | null;
  view: { frame: Frame; me: Me | null; match: MatchView };
}

async function call<T>(method: "GET" | "POST", path: string, body?: unknown): Promise<T> {
  const res = await fetch(path, {
    method,
    // The server refuses a POST without this header, so other sites cannot drive the player's unit.
    headers: method === "POST" ? { "Content-Type": "application/json", "X-IBC-Play": "1" } : {},
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`${method} ${path}: HTTP ${res.status} ${await res.text()}`);
  return (await res.json()) as T;
}

export const api = {
  setup: () => call<Setup>("GET", "setup"),
  /** The state now, or (with `after`) as soon as a later tick has run, after a wait of at most 2 s. */
  state: (after?: number) => call<State>("GET", after === undefined ? "state" : `state?after=${after}`),
  input: (msg: InputMsg) => call<{ ok: boolean }>("POST", "input", msg),
  step: (msg?: InputMsg) => call<State>("POST", "step", msg),
  run: (running: boolean) => call<State>("POST", "run", { running }),
  possess: (slot: number | null) => call<State>("POST", "possess", { slot }),
  quit: () => call<{ ok: boolean }>("POST", "quit"),
};
