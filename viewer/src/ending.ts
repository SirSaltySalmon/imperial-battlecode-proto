// How a round ends on screen: the winning side's headline and, after a detonation, the blast. Drawn by the
// viewer alone, from the frames it already has, from the round's end until `ENDING_TICKS` after it; replays
// carry nothing of it.

import type { Pen } from "./render";
import { FONT } from "./render";
import { alpha, type Palette } from "./theme";
import type { Timeline } from "./timeline";
import { EVENT_DETONATE, EVENT_ROUND_END, ROUND_WIN_REASONS, TILE, type Frame, type Vec2 } from "./types";

/** Ticks a round's ending stays on screen after the round's last frame. */
export const ENDING_TICKS = 5;

export interface Ending {
  round: number;
  /** Team that won the round, and whether it was attacking. */
  winner: number;
  attackers: boolean;
  /** `ROUND_WIN_*` id, 0 when unknown. */
  reason: number;
  /** Score once the round is counted. */
  score: [number, number];
  /** Where the charge went off (fixed-point units), when the round ended by detonation. */
  blast: Vec2 | null;
}

/** A replay round's ending and the frames it covers: from the round-end event to the round's last frame. */
export interface RoundEnding extends Ending {
  from: number;
  to: number;
}

/** The ending of every round of a replay that has a winner, in order. `scoreAt` reads a frame's score. */
export function roundEndings(timeline: Timeline, scoreAt: (frame: number) => [number, number]): RoundEnding[] {
  const out: RoundEnding[] = [];
  for (const span of timeline.rounds) {
    if (span.winner === null || span.winner < 0) continue;
    const ev = timeline.events.filter((e) => e.round === span.round);
    const end = ev.find((e) => e.kind === EVENT_ROUND_END);
    const boom = ev.find((e) => e.kind === EVENT_DETONATE);
    out.push({
      round: span.round,
      winner: span.winner,
      attackers: span.winner === span.attackers,
      reason: span.reason,
      score: scoreAt(span.end),
      blast: boom ? [boom.x, boom.y] : null,
      from: end ? end.frame : span.end,
      to: span.end,
    });
  }
  return out;
}

/** The ending on screen at frame `i`, if any. */
export function endingAt(endings: readonly RoundEnding[], i: number): RoundEnding | null {
  return endings.find((e) => e.from <= i && i <= e.to) ?? null;
}

/** Ticks a replay's playback holds on frame `i`: `ENDING_TICKS` on the last frame of a round that ended. */
export function holdAt(endings: readonly RoundEnding[], i: number): number {
  return endings.some((e) => e.to === i) ? ENDING_TICKS : 0;
}

/**
 * Follows a live match frame by frame (`ibc play`, where ticks can be skipped between polls) and notes each
 * round's ending. A round is counted when the score changes, which no skipped tick can hide; the round-end
 * and detonation events add the reason and the blast when they arrived.
 */
export class EndingTracker {
  /** The latest round's ending and the tick it was noted on. */
  last: { ending: Ending; tick: number } | null = null;
  private score: [number, number] | null = null;
  /** What the events since the last counted round said. */
  private reason = 0;
  private blast: Vec2 | null = null;
  private tick = -1;

  /** Note a frame; a tick already noted (the page can be handed the same state twice) is skipped. */
  note(frame: Frame): void {
    if (frame.tick <= this.tick) return;
    this.tick = frame.tick;
    for (const e of frame.events) {
      if (e.kind === EVENT_ROUND_END) this.reason = e.value;
      if (e.kind === EVENT_DETONATE) this.blast = [e.x, e.y];
    }
    const before = this.score;
    this.score = [frame.score[0], frame.score[1]];
    if (!before || sameScore(before, frame.score)) return;
    const winner = frame.score[0] > before[0] ? 0 : 1;
    const ending: Ending = {
      round: frame.round,
      winner,
      attackers: winner === frame.attackers,
      reason: this.reason,
      score: this.score,
      blast: this.blast,
    };
    this.last = { ending, tick: frame.tick };
    this.reason = 0;
    this.blast = null;
  }

  /** The ending to show at `tick`: the latest round's, until `ENDING_TICKS` after it (for good once `over`). */
  at(tick: number, over: boolean): { ending: Ending; tick: number } | null {
    const l = this.last;
    return l && (over || tick - l.tick <= ENDING_TICKS) ? l : null;
  }
}

function sameScore(a: readonly number[], b: readonly number[]): boolean {
  return a[0] === b[0] && a[1] === b[1];
}

/**
 * The blast: everything within `radius` (fixed-point units) of the charge, with a flash at its centre. `t` is
 * ticks since the round ended.
 */
export function drawBlast(pen: Pen, at: Vec2, radius: number | undefined, t: number): void {
  const { ctx, px } = pen;
  const pal = pen.scene.palette;
  const x = at[0] / TILE;
  const y = at[1] / TILE;
  // The flash grows out over the first tick and then holds, fading slightly.
  const grow = Math.min(1, Math.max(0, t));
  const progress = Math.min(1, Math.max(0, t / ENDING_TICKS));
  if (radius !== undefined && radius > 0) {
    const r = radius / TILE;
    ctx.beginPath();
    ctx.arc(x, y, r * grow, 0, Math.PI * 2);
    ctx.fillStyle = alpha(pal.charge, 0.16 - 0.06 * progress);
    ctx.fill();
    ctx.beginPath();
    ctx.arc(x, y, r, 0, Math.PI * 2);
    ctx.strokeStyle = alpha(pal.charge, 0.85);
    ctx.lineWidth = 2 * px;
    ctx.setLineDash([8 * px, 5 * px]);
    ctx.stroke();
    ctx.setLineDash([]);
  }
  ctx.beginPath();
  ctx.arc(x, y, Math.max(0.6, 14 * px) * (0.4 + 0.6 * grow), 0, Math.PI * 2);
  ctx.fillStyle = alpha(pal.warn, 0.9 - 0.4 * progress);
  ctx.fill();
}

/** The headline, centred in the upper middle of the stage, in CSS pixels. */
export function drawHeadline(
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number,
  ending: Ending,
  names: [string, string],
  pal: Palette,
): void {
  const title = ending.attackers ? "ATTACKERS WIN" : "DEFENDERS WIN";
  const how = ROUND_WIN_REASONS[ending.reason];
  const who = names[ending.winner] ?? `team ${"AB"[ending.winner] ?? ending.winner}`;
  const sub = [who, how, `${ending.score[0]}:${ending.score[1]}`].filter(Boolean).join(" · ");
  const size = Math.round(Math.max(22, Math.min(52, width / 16)));
  const subSize = Math.round(size * 0.36);
  const cy = height * 0.22;
  const barH = size * 1.25 + subSize * 1.8;
  ctx.fillStyle = alpha(pal.bg, 0.78);
  ctx.fillRect(0, cy - barH / 2, width, barH);
  ctx.fillStyle = pal.team[ending.winner] ?? pal.text;
  ctx.fillRect(0, cy - barH / 2, width, 2);
  ctx.fillRect(0, cy + barH / 2 - 2, width, 2);
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.font = `800 ${size}px ${FONT}`;
  ctx.fillText(title, width / 2, cy - subSize * 0.7);
  ctx.font = `500 ${subSize}px ${FONT}`;
  ctx.fillStyle = pal.text;
  ctx.fillText(sub, width / 2, cy + size * 0.55);
}
