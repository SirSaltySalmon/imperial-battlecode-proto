import { describe, expect, it } from "vitest";
import { ENDING_TICKS, EndingTracker, endingAt, holdAt, roundEndings } from "../src/ending";
import type { Timeline } from "../src/timeline";
import { EVENT_DETONATE, EVENT_KILL, EVENT_ROUND_END, type EventFrame } from "../src/types";
import { frame } from "./helpers";

const ev = (kind: number, actor: number, value = 0, x = 0, y = 0): EventFrame => ({ kind, actor, target: -1, x, y, value });
const at = (e: EventFrame, f: number, round: number) => ({ ...e, frame: f, round });

// Round 1 (team 0 attacks) ends by detonation at frame 4; round 2 (team 1 attacks) by elimination at frame 9.
const timeline = {
  frames: 12,
  rounds: [
    { round: 0, start: 0, end: 0, attackers: 0, winner: null, reason: 0, plant: -1 },
    { round: 1, start: 1, end: 6, attackers: 0, winner: 0, reason: 2, plant: 2 },
    { round: 2, start: 7, end: 11, attackers: 1, winner: 0, reason: 1, plant: -1 },
  ],
  events: [
    at(ev(EVENT_DETONATE, -1, 0, 4096, 2048), 4, 1),
    at(ev(EVENT_ROUND_END, 0, 2), 4, 1),
    at(ev(EVENT_KILL, 0), 9, 2),
    at(ev(EVENT_ROUND_END, 0, 1), 9, 2),
  ],
} as Timeline;
const scores: Record<number, [number, number]> = { 6: [1, 0], 11: [2, 0] };

describe("round endings in a replay", () => {
  const endings = roundEndings(timeline, (i) => scores[i]);

  it("has one per round that was won: the side, the reason, the score and any blast", () => {
    expect(endings).toEqual([
      { round: 1, winner: 0, attackers: true, reason: 2, score: [1, 0], blast: [4096, 2048], from: 4, to: 6 },
      { round: 2, winner: 0, attackers: false, reason: 1, score: [2, 0], blast: null, from: 9, to: 11 },
    ]);
  });

  it("shows from the round-end event to the round's last frame, and holds playback there", () => {
    expect(endingAt(endings, 3)).toBeNull();
    expect(endingAt(endings, 4)?.round).toBe(1);
    expect(endingAt(endings, 6)?.round).toBe(1);
    expect(endingAt(endings, 7)).toBeNull();
    expect(endingAt(endings, 10)?.round).toBe(2);
    expect(holdAt(endings, 6)).toBe(ENDING_TICKS);
    expect(holdAt(endings, 5)).toBe(0);
    expect(holdAt(endings, 11)).toBe(ENDING_TICKS);
  });
});

describe("round endings in a live match", () => {
  it("counts a round when the score changes, with the reason and blast its events gave", () => {
    const t = new EndingTracker();
    t.note(frame({ tick: 1, round: 1, attackers: 1, score: [0, 0] }));
    t.note(frame({ tick: 2, round: 1, attackers: 1, score: [0, 0], events: [ev(EVENT_DETONATE, -1, 0, 7, 8)] }));
    t.note(frame({ tick: 3, round: 1, attackers: 1, score: [0, 1], events: [ev(EVENT_ROUND_END, 1, 2)] }));
    expect(t.last).toEqual({
      ending: { round: 1, winner: 1, attackers: true, reason: 2, score: [0, 1], blast: [7, 8] },
      tick: 3,
    });
    expect(t.at(3 + ENDING_TICKS, false)).not.toBeNull();
    expect(t.at(4 + ENDING_TICKS, false)).toBeNull();
    expect(t.at(4 + ENDING_TICKS, true)).not.toBeNull();
  });

  it("still counts a round whose events were skipped, and does not carry a blast into the next round", () => {
    const t = new EndingTracker();
    t.note(frame({ tick: 1, round: 1, attackers: 0, score: [0, 0], events: [ev(EVENT_DETONATE, -1)] }));
    t.note(frame({ tick: 9, round: 2, attackers: 0, score: [1, 0] }));
    expect(t.last?.ending).toMatchObject({ winner: 0, attackers: true, reason: 0 });
    // The same state handed over twice is not noted twice.
    t.note(frame({ tick: 9, round: 2, attackers: 0, score: [1, 0], events: [ev(EVENT_DETONATE, -1)] }));
    t.note(frame({ tick: 20, round: 3, attackers: 0, score: [1, 1] }));
    expect(t.last?.ending).toMatchObject({ winner: 1, attackers: false, blast: null });
  });
});
