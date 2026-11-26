import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { describeRound, historyAt, roundResults } from "../src/panels/history";
import { feedEntries, killCause, killCauses, type FeedOptions } from "../src/panels/killfeed";
import { parseShopRules } from "../src/panels/shared";
import { TAB_COLUMNS, tabCells, tabUnits } from "../src/panels/tab";
import type { RoundSpan, TimedEvent, Timeline } from "../src/timeline";
import {
  EFFECT_FIRE,
  EFFECT_SMOKE,
  EVENT_DETONATE,
  EVENT_KILL,
  EVENT_ROUND_END,
  S_ALIVE,
  S_HAS_CHARGE,
  type LoadoutFrame,
} from "../src/types";
import { rulesFor } from "../src/vision";
import { frame, unit } from "./helpers";

const shop = parseShopRules(JSON.parse(readFileSync(new URL("./fixtures/rules.json", import.meta.url), "utf8")))!;
const view = rulesFor(null);

function loadout(id: number, over: Partial<LoadoutFrame> = {}): LoadoutFrame {
  return {
    id,
    weapons: [-1, 0],
    ammo: [0, 12],
    slot: 1,
    charges: [0, 0],
    cooldown: [0, 0],
    has_kit: false,
    damage: 0,
    faulted: false,
    ...over,
  };
}

describe("team tab", () => {
  // Team A = units 0, 1; team B = units 2, 3.
  const f = frame({
    units: [
      unit(2, 1, { kit: 4, credits: 3100, kills: 1 }),
      unit(0, 0, { kit: 1, hp: 61, armor: 25, credits: 150, state: S_ALIVE | S_HAS_CHARGE }),
      unit(1, 0, { alive: false, hp: 0, state: 0, deaths: 1 }),
      unit(3, 1, { kit: 2 }),
    ],
    loadout: [
      loadout(2, { has_kit: true, damage: 100 }),
      loadout(0, { weapons: [1, 0], ammo: [5, 12], slot: 0, charges: [2, 1], cooldown: [0, 3], damage: 44 }),
      loadout(1),
      loadout(3, { charges: [1, 0] }),
    ],
  });

  it("has one row per unit, team A first, with kit, health, weapons and charges", () => {
    const rows = tabUnits(f, shop, view, 2);
    expect(rows.map((r) => r.label)).toEqual(["A1", "A2", "B1", "B2"]);
    const a1 = rows[0];
    expect(a1).toMatchObject({ kit: "lancer", alive: true, hp: 61, maxHp: 90, armor: 25, credits: 150 });
    expect(a1.weapons).toEqual([
      { name: "rifle", ammo: 5, magazine: 8, inHand: true },
      { name: "pistol", ammo: 12, magazine: 12, inHand: false },
    ]);
    expect(a1.abilities).toEqual([
      { name: "dash", charges: 2, max: 2, cooldown: 0 },
      { name: "flash", charges: 1, max: 2, cooldown: 3 },
    ]);
    expect(a1.carriesCharge).toBe(true);
    expect(rows[1].alive).toBe(false);
    expect(rows[2]).toMatchObject({ kit: "bulwark", maxHp: 115, defuseKit: true, damage: 100 });
  });

  it("renders a cell per column", () => {
    const cells = tabCells(tabUnits(f, shop, view, 2)[0]);
    expect(cells).toHaveLength(TAB_COLUMNS.length);
    expect(cells[TAB_COLUMNS.indexOf("weapons")]).toBe("▸ rifle 5/8  pistol 12/12");
    expect(cells[TAB_COLUMNS.indexOf("abilities")]).toBe("dash 2/2  flash 1/2 (cd 3)");
    expect(cells[TAB_COLUMNS.indexOf("gear")]).toBe("charge");
    expect(cells[TAB_COLUMNS.indexOf("hp")]).toBe("61 / 90");
    expect(tabCells(tabUnits(f, shop, view, 2)[1])[TAB_COLUMNS.indexOf("status")]).toBe("dead");
  });

  it("falls back to the weapon in hand for engine builds without loadouts", () => {
    const rows = tabUnits({ ...f, loadout: undefined }, null, view, 2);
    expect(rows[0].weapons).toEqual([{ name: "pistol", ammo: 12, inHand: true }]);
    expect(rows[0].abilities).toEqual([]);
    expect(rows[0].maxHp).toBe(view.max_hp);
  });
});

function span(round: number, start: number, end: number, attackers = 0): RoundSpan {
  return { round, start, end, attackers, winner: null, reason: 0, plant: -1 };
}

function ev(kind: number, frameIndex: number, round: number, over: Partial<TimedEvent> = {}): TimedEvent {
  return { kind, actor: 0, target: 2, x: 0, y: 0, value: 1, frame: frameIndex, round, ...over };
}

const kill = (frameIndex: number, target: number, round = 1) => ev(EVENT_KILL, frameIndex, round, { target });

describe("kill feed", () => {
  const opts: FeedOptions = { ticks: 24, fade: 8, rows: 6 };
  const timeline: Timeline = {
    frames: 200,
    rounds: [span(0, 0, 0), span(1, 1, 99), span(2, 100, 199)],
    events: [kill(5, 2), kill(10, 3), kill(12, 4), kill(30, 5), kill(105, 6, 2)],
  };
  const name = (e: TimedEvent) => `w${e.value}`;

  it("lists recent kills oldest first", () => {
    expect(feedEntries(timeline, 12, name, opts).map((e) => e.victim)).toEqual([2, 3, 4]);
    expect(feedEntries(timeline, 4, name, opts)).toEqual([]);
  });

  it("fades entries out over replay ticks and drops them after", () => {
    const at30 = feedEntries(timeline, 30, name, opts);
    expect(at30.map((e) => [e.victim, e.age, e.opacity])).toEqual([
      [3, 20, 0.5],
      [4, 18, 0.75],
      [5, 0, 1],
    ]);
    expect(feedEntries(timeline, 53, name, opts).map((e) => e.victim)).toEqual([5]);
    expect(feedEntries(timeline, 54, name, opts)).toEqual([]);
  });

  it("starts empty each round and keeps at most `rows` entries", () => {
    expect(feedEntries(timeline, 110, name, opts).map((e) => e.victim)).toEqual([6]);
    expect(feedEntries(timeline, 30, name, { ...opts, rows: 2 }).map((e) => e.victim)).toEqual([4, 5]);
  });

  it("names the weapon, or the ability whose effect covers the victim", () => {
    expect(killCause(ev(EVENT_KILL, 1, 1, { value: 1 }), [], view, shop)).toBe("rifle");
    const fire = { kind: EFFECT_FIRE, team: 0, x: 5000, y: 5000, radius: 1536, ticks_left: 3, x2: 0, y2: 0 };
    const smoke = { ...fire, kind: EFFECT_SMOKE, radius: 4000 };
    const burning = frame({ units: [unit(0, 0), unit(2, 1)], effects: [smoke, fire] });
    const burntOut = frame({ units: [unit(0, 0), unit(2, 1)], effects: [smoke] });
    const fireKill = ev(EVENT_KILL, 1, 1, { value: -1, x: 5800, y: 5000 });
    // Fire is preferred over a smoke covering the same spot.
    expect(killCause(fireKill, [burning], view, shop)).toBe("incendiary");
    // A fire gone by the kill's frame is still found in the frame before.
    expect(killCause(fireKill, [burning, burntOut], view, shop)).toBe("incendiary");
    expect(killCause({ ...fireKill, x: 8800 }, [burning], view, shop)).toBe("smoke");
    expect(killCause({ ...fireKill, x: 20000 }, [burning], view, shop)).toBe("ability");
    expect(killCause({ ...fireKill, actor: -1 }, [burning], view, shop)).toBe("incendiary");
    expect(killCause({ ...fireKill, actor: -1, x: 20000 }, [burning], view, shop)).toBe("the world");
    // Detonation kills everyone in the blast radius: no killer, no weapon, on the detonation tick.
    const boom = { ...burntOut, events: [{ kind: EVENT_DETONATE, actor: -1, target: -1, x: 0, y: 0, value: 0 }] };
    expect(killCause({ ...fireKill, actor: -1, x: 20000 }, [burning, boom], view, shop)).toBe("blast");
    const causes = killCauses(
      { frames: 2, rounds: [span(1, 0, 1)], events: [fireKill] },
      { parse: (i) => (i === 0 ? burning : burntOut) },
      view,
      shop,
    );
    expect(causes.get("1:2")).toBe("incendiary");
  });
});

describe("round history", () => {
  const timeline: Timeline = {
    frames: 300,
    rounds: [span(0, 0, 0), span(1, 1, 99, 0), span(2, 100, 199, 1), span(3, 200, 299, 0)],
    events: [
      ev(EVENT_ROUND_END, 97, 1, { actor: 1, value: 1 }),
      kill(150, 2, 2),
      ev(EVENT_ROUND_END, 198, 2, { actor: 1, value: 3 }),
    ],
  };

  it("lists rounds decided by the frame shown", () => {
    const results = roundResults(timeline);
    expect(results.map(describeRound)).toEqual([
      "R1 B (defence) by elimination",
      "R2 B (attack) by defuse",
    ]);
    expect(historyAt(results, 96)).toEqual([]);
    expect(historyAt(results, 97).map((r) => r.round)).toEqual([1]);
    expect(historyAt(results, 299).map((r) => r.round)).toEqual([1, 2]);
  });
});
