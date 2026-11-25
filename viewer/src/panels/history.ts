// Round history: who won each round so far and how. Shown as a strip in the side panel at all times and as
// a table in the team tab. Only rounds decided at the frame being shown are listed, so scrubbing back hides
// results that have not happened yet.

import type { Timeline } from "../timeline";
import { EVENT_ROUND_END, ROUND_WIN_REASONS, type Header } from "../types";
import { el } from "../ui";

export interface RoundResult {
  round: number;
  /** Team that attacked. */
  attackers: number;
  /** Winning team, or -1 for a round that ended without one. */
  winner: number;
  reason: string;
  /** Frame of the round's end event. */
  frame: number;
}

/** Every decided round of the match, in order. Built once per replay. */
export function roundResults(timeline: Timeline): RoundResult[] {
  const out: RoundResult[] = [];
  for (const e of timeline.events) {
    if (e.kind !== EVENT_ROUND_END) continue;
    const span = timeline.rounds.find((r) => r.round === e.round);
    out.push({
      round: e.round,
      attackers: span?.attackers ?? -1,
      winner: e.actor,
      reason: ROUND_WIN_REASONS[e.value] ?? "decision",
      frame: e.frame,
    });
  }
  return out;
}

/** The rounds decided at or before `frame`. */
export function historyAt(results: RoundResult[], frame: number): RoundResult[] {
  return results.filter((r) => r.frame <= frame);
}

export const teamLetter = (team: number): string => (team === 0 ? "A" : team === 1 ? "B" : "-");
export const teamClass = (team: number): string => (team === 0 ? "a" : team === 1 ? "b" : "");
export const teamName = (header: Header, team: number): string =>
  header.bots[team]?.name || `Team ${teamLetter(team)}`;

/** One line of text for a round: "R3 A (attack) by elimination". */
export function describeRound(r: RoundResult): string {
  if (r.winner < 0) return `R${r.round} no winner (${r.reason})`;
  const role = r.winner === r.attackers ? "attack" : "defence";
  return `R${r.round} ${teamLetter(r.winner)} (${role}) by ${r.reason}`;
}

/** The always-visible strip: one chip per decided round, coloured by the winner. */
export class RoundStrip {
  private key = "";

  constructor(private readonly root: HTMLElement) {}

  update(results: RoundResult[], frame: number): void {
    const shown = historyAt(results, frame);
    const key = shown.map((r) => r.round).join(",");
    if (key === this.key && this.root.childElementCount > 0) return;
    this.key = key;
    const chips = shown.map((r) => {
      const chip = el("span", `round-chip ${teamClass(r.winner)}`, String(r.round));
      chip.title = describeRound(r);
      return chip;
    });
    this.root.replaceChildren(
      el("div", "section-title", "Rounds"),
      el("div", "round-strip", ...(chips.length ? chips : [el("span", "note", "none decided yet")])),
    );
  }
}
