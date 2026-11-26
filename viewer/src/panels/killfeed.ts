// The kill feed over the map: recent kills of the current round, killer, what killed, victim. Entries fade
// out over replay time (ticks), not wall time, so pausing freezes the feed and scrubbing rebuilds it.
// Kills are public events, so the feed is the same in every perspective.

import { unitLabel } from "../draw-units";
import type { FrameStore } from "../engine";
import { recentEvents, roundIndexAt, type TimedEvent, type Timeline } from "../timeline";
import { EFFECT_FIRE, EVENT_KILL, type Frame } from "../types";
import { el } from "../ui";
import { weaponName, type ViewRules } from "../vision";
import { teamClass } from "./history";
import { EFFECT_NAMES, num, type ShopRules } from "./shared";

export interface FeedOptions {
  /** Ticks an entry stays up. At 1x (8 ticks a second) 24 ticks is three seconds. */
  ticks: number;
  /** Ticks over which it fades out at the end of that time. */
  fade: number;
  /** Most entries shown at once (the oldest go first). */
  rows: number;
}

export const FEED: FeedOptions = { ticks: 24, fade: 8, rows: 6 };

export interface FeedEntry {
  key: string;
  killer: number;
  victim: number;
  how: string;
  /** Ticks since the kill. */
  age: number;
  opacity: number;
}

/**
 * Kills of the current round in the last `opts.ticks` frames, oldest first, at most `opts.rows`.
 * `how` names what made each kill (see `killCauses`).
 */
export function feedEntries(
  timeline: Timeline,
  frame: number,
  how: (e: TimedEvent) => string,
  opts: FeedOptions = FEED,
): FeedEntry[] {
  const round = timeline.rounds[roundIndexAt(timeline, frame)];
  return recentEvents(timeline, frame, opts.ticks, [EVENT_KILL])
    .filter((e) => e.frame >= round.start)
    .slice(-opts.rows)
    .map((e) => {
      const age = frame - e.frame;
      const left = opts.ticks - age;
      return {
        key: `${e.frame}:${e.target}`,
        killer: e.actor,
        victim: e.target,
        how: how(e),
        age,
        opacity: opts.fade > 0 ? Math.max(0, Math.min(1, left / opts.fade)) : 1,
      };
    });
}

/** Effect kinds that deal damage (`abilities/effects.rs`: only fire burns). Preferred when several cover. */
const DAMAGING_EFFECTS = [EFFECT_FIRE];

/**
 * What made a kill. A weapon kill names the weapon. A kill without one (`value` -1) came from an ability's
 * effect: the effect covering the victim (of the killer's team, when there is a killer; a damaging kind
 * first) names the abilities that make it. `frames` are searched in order: the frame before the kill, where
 * an effect that burns out on the killing tick is still listed, then the kill's own frame. Fire hurts its
 * own side too; such a kill has no killer.
 */
export function killCause(e: TimedEvent, frames: Frame[], view: ViewRules, shop: ShopRules | null): string {
  if (e.value >= 0) return weaponName(view, e.value);
  const team = e.actor >= 0 ? frames.flatMap((f) => f.units).find((u) => u.id === e.actor)?.team : undefined;
  const covering = frames
    .flatMap((f) => f.effects)
    .filter(
      (fx) =>
        (team === undefined || fx.team === team) && Math.hypot(fx.x - e.x, fx.y - e.y) <= fx.radius + view.unit_radius,
    );
  const effect = covering.find((fx) => DAMAGING_EFFECTS.includes(fx.kind)) ?? covering[0];
  if (!effect) return e.actor < 0 ? "the world" : "ability";
  const names = (shop?.abilities ?? []).filter((a) => num(a, "effect") === effect.kind).map((a) => a.name);
  return names.length ? names.join(" / ") : (EFFECT_NAMES[effect.kind] ?? "ability");
}

/** `killCause` for every kill of the match, keyed like `FeedEntry.key`. One pass at load. */
export function killCauses(
  timeline: Timeline,
  store: Pick<FrameStore, "parse">,
  view: ViewRules,
  shop: ShopRules | null,
): Map<string, string> {
  const out = new Map<string, string>();
  for (const e of timeline.events) {
    if (e.kind !== EVENT_KILL) continue;
    const frames = e.value < 0 ? [Math.max(0, e.frame - 1), e.frame].map((i) => store.parse(i)) : [];
    out.set(`${e.frame}:${e.target}`, killCause(e, frames, view, shop));
  }
  return out;
}

function unitSpan(id: number, teamSize: number): HTMLElement {
  if (id < 0) return el("span", "", "the world");
  const team = id < teamSize ? 0 : 1;
  return el("span", teamClass(team), unitLabel({ id, team }, teamSize));
}

export class KillFeed {
  private key = "";
  private rows: HTMLElement[] = [];

  constructor(private readonly root: HTMLElement) {}

  clear(): void {
    this.key = "";
    this.rows = [];
    this.root.replaceChildren();
  }

  update(timeline: Timeline, frame: number, teamSize: number, causes: Map<string, string>): void {
    const entries = feedEntries(timeline, frame, (e) => causes.get(`${e.frame}:${e.target}`) ?? "");
    const key = entries.map((e) => e.key).join(",");
    if (key !== this.key) {
      this.key = key;
      this.rows = entries.map((e) =>
        el("div", "feed-row", unitSpan(e.killer, teamSize), el("span", "how", e.how), unitSpan(e.victim, teamSize)),
      );
      this.root.replaceChildren(...this.rows);
    }
    entries.forEach((e, i) => (this.rows[i].style.opacity = String(e.opacity)));
  }
}
