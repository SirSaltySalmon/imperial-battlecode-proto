// A small index of the whole match, built in one pass over the frames: where rounds start and how they end,
// and the public events (kills, plant, defuse, detonation). Everything else is read from frames on demand.

import type { FrameStore } from "./engine";
import { EVENT_DAMAGE, EVENT_PLANT, EVENT_ROUND_END, type EventFrame } from "./types";

export interface RoundSpan {
  round: number;
  /** First and last frame index of the round (inclusive). */
  start: number;
  end: number;
  /** Team that attacked. */
  attackers: number;
  /** Winning team, or null while unknown (match stopped mid-round). */
  winner: number | null;
  /** `ROUND_WIN_*` id, or 0. */
  reason: number;
  /** Frame of the plant, or -1. */
  plant: number;
}

export interface TimedEvent extends EventFrame {
  frame: number;
  round: number;
}

export interface Timeline {
  frames: number;
  rounds: RoundSpan[];
  /** All events except damage, in frame order. */
  events: TimedEvent[];
}

export function buildTimeline(store: FrameStore): Timeline {
  const rounds: RoundSpan[] = [];
  const events: TimedEvent[] = [];
  let span: RoundSpan | null = null;
  for (let i = 0; i < store.count; i++) {
    const f = store.parse(i);
    if (!span || f.round !== span.round) {
      span = { round: f.round, start: i, end: i, attackers: f.attackers, winner: null, reason: 0, plant: -1 };
      rounds.push(span);
    }
    span.end = i;
    for (const e of f.events) {
      if (e.kind === EVENT_DAMAGE) continue;
      events.push({ ...e, frame: i, round: f.round });
      if (e.kind === EVENT_PLANT && span.plant < 0) span.plant = i;
      if (e.kind === EVENT_ROUND_END) {
        span.winner = e.actor;
        span.reason = e.value;
      }
    }
  }
  return { frames: store.count, rounds, events };
}

/** Index into `timeline.rounds` of the round that contains `frame`. */
export function roundIndexAt(timeline: Timeline, frame: number): number {
  let lo = 0;
  let hi = timeline.rounds.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (timeline.rounds[mid].start <= frame) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

/** Events of `kinds` in frames `(frame - window, frame]`, oldest first. */
export function recentEvents(timeline: Timeline, frame: number, window: number, kinds: number[]): TimedEvent[] {
  const ev = timeline.events;
  // First event after `frame`.
  let lo = 0;
  let hi = ev.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (ev[mid].frame <= frame) lo = mid + 1;
    else hi = mid;
  }
  const out: TimedEvent[] = [];
  for (let i = lo - 1; i >= 0 && ev[i].frame > frame - window; i--) {
    if (kinds.includes(ev[i].kind)) out.push(ev[i]);
  }
  return out.reverse();
}
