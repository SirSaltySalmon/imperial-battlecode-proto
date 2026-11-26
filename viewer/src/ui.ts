// The page around the canvas: side panel, round banner, scrub-bar markers (the kill feed is in panels/). Built with plain DOM
// calls; all replay-supplied text (bot names, map name) goes through `textContent`.

import { stateTags, unitLabel } from "./draw-units";
import { siteName, type GameMap } from "./map";
import type { Perspective } from "./perspective";
import { recentEvents, roundIndexAt, type TimedEvent, type Timeline } from "./timeline";
import {
  EVENT_DEFUSE,
  EVENT_DETONATE,
  EVENT_PLANT,
  PHASE_ACTION,
  PHASE_BUY,
  PHASE_DRAFT,
  PHASE_MATCH_END,
  PHASE_PLANTED,
  PHASE_ROUND_END,
  ROUND_WIN_REASONS,
  S_HAS_CHARGE,
  S_HAS_KIT,
  type Frame,
  type Header,
  type UnitFrame,
} from "./types";
import { weaponName, type ViewRules } from "./vision";

type Child = Node | string;

export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className = "",
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (className) e.className = className;
  e.append(...children);
  return e;
}

const PHASE_NAMES: Record<number, string> = {
  [PHASE_DRAFT]: "draft",
  [PHASE_BUY]: "buy phase",
  [PHASE_ACTION]: "action",
  [PHASE_PLANTED]: "charge planted",
  [PHASE_ROUND_END]: "round over",
  [PHASE_MATCH_END]: "match over",
};

const teamLetter = (team: number) => (team === 0 ? "A" : "B");
const teamClass = (team: number) => (team === 0 ? "a" : "b");

export interface PanelActions {
  setPerspective(p: Perspective): void;
  toggle(layer: "vision" | "sounds" | "debug"): void;
}

interface UnitRow {
  root: HTMLButtonElement;
  hp: HTMLElement;
  hpBar: HTMLElement;
  armor: HTMLElement;
  info: HTMLElement;
  money: HTMLElement;
  kd: HTMLElement;
}

/** The side panel. Built once per replay; `update` only touches text and bar widths. */
export class Panel {
  private readonly points: HTMLElement[] = [];
  private readonly sides: HTMLElement[] = [];
  private readonly credits: HTMLElement[] = [];
  private readonly phase = el("b");
  private readonly clock = el("span");
  private readonly rows: UnitRow[] = [];
  private readonly viewButtons: HTMLButtonElement[] = [];
  private readonly layerButtons = new Map<string, HTMLButtonElement>();

  constructor(
    root: HTMLElement,
    header: Header,
    map: GameMap,
    first: Frame,
    private readonly rules: ViewRules,
    private readonly actions: PanelActions,
  ) {
    root.replaceChildren();
    const teamSize = first.units.length / 2;

    const score = el("div", "score");
    for (const team of [0, 1]) {
      const name = el("div", `name t${teamClass(team)}`, header.bots[team].name || `team ${teamLetter(team)}`);
      name.title = `Team ${teamLetter(team)}: ${header.bots[team].name}`;
      const side = el("div", "side");
      const pts = el("div", "points", "0");
      this.sides.push(side);
      this.points.push(pts);
      score.append(el("div", "", name, pts, side));
      if (team === 0) score.append(el("div", "points", ":"));
    }
    root.append(score, el("div", "phase", this.phase, this.clock));

    const views = el("div", "seg");
    const options: [string, Perspective][] = [
      ["Everyone", { kind: "all" }],
      ["Team A", { kind: "team", team: 0 }],
      ["Team B", { kind: "team", team: 1 }],
    ];
    for (const [text, p] of options) {
      const b = el("button", "", text);
      b.addEventListener("click", () => actions.setPerspective(p));
      this.viewButtons.push(b);
      views.append(b);
    }
    root.append(el("div", "", el("div", "section-title", "View as"), views));

    for (const team of [0, 1]) {
      const credits = el("small");
      this.credits.push(credits);
      const box = el("div", "", el("div", `team-head t${teamClass(team)}`, `Team ${teamLetter(team)}`, credits));
      for (const u of first.units.filter((x) => x.team === team)) {
        box.append(this.buildRow(u, teamSize));
      }
      root.append(box);
    }

    const layers = el("div", "seg");
    for (const [key, text] of [["vision", "Vision"], ["sounds", "Sounds"], ["debug", "Debug"]] as const) {
      const b = el("button", "", text);
      b.addEventListener("click", () => actions.toggle(key));
      this.layerButtons.set(key, b);
      layers.append(b);
    }
    root.append(el("div", "", el("div", "section-title", "Layers"), layers));

    const r = header.result;
    root.append(
      el(
        "div",
        "meta",
        `map ${map.name} (${map.width}x${map.height})`,
        el("br"),
        `seed ${header.seed} · engine ${header.engine_version}`,
        el("br"),
        `${r.ticks} ticks · final ${r.score[0]}:${r.score[1]}${r.finished ? "" : " (unfinished)"}`,
      ),
    );
  }

  private buildRow(u: UnitFrame, teamSize: number): HTMLElement {
    const label = unitLabel(u, teamSize);
    const row: UnitRow = {
      root: el("button", "unit"),
      hp: el("i"),
      hpBar: el("div", "bar"),
      armor: el("i"),
      info: el("div", "info"),
      money: el("div", "money"),
      kd: el("div", "kd"),
    };
    row.hpBar.append(row.hp);
    row.root.title = `See what ${label} sees`;
    row.root.append(
      el("div", `chip ${teamClass(u.team)}`, label.slice(1)),
      el("div", "bars", row.hpBar, el("div", "bar thin", row.armor)),
      row.money,
      row.info,
      row.kd,
    );
    row.root.addEventListener("click", () => this.actions.setPerspective({ kind: "unit", id: u.id }));
    this.rows[u.id] = row;
    return row.root;
  }

  update(frame: Frame, perspective: Perspective, show: Record<string, boolean>): void {
    for (const team of [0, 1]) {
      this.points[team].textContent = String(frame.score[team]);
      this.sides[team].textContent = frame.attackers === team ? "attacking" : "defending";
      const total = frame.units.filter((u) => u.team === team).reduce((n, u) => n + u.credits, 0);
      this.credits[team].textContent = `${total} credits`;
    }
    this.phase.textContent = frame.round > 0 ? `Round ${frame.round} · ${PHASE_NAMES[frame.phase] ?? ""}` : "Draft";
    this.clock.textContent = frame.phase === PHASE_MATCH_END ? "" : `${frame.phase_left} ticks left`;
    for (const u of frame.units) {
      const row = this.rows[u.id];
      if (!row) continue;
      const hp = Math.max(0, Math.min(1, u.hp / this.rules.max_hp));
      row.root.classList.toggle("dead", !u.alive);
      row.root.classList.toggle("selected", perspective.kind === "unit" && perspective.id === u.id);
      row.hp.style.width = `${hp * 100}%`;
      row.hpBar.classList.toggle("low", hp < 0.35);
      row.armor.style.width = `${Math.min(1, u.armor / 50) * 100}%`;
      row.money.textContent = `${u.credits} cr`;
      row.kd.textContent = `${u.kills} K / ${u.deaths} D`;
      const tags = stateTags(u);
      if (u.state & S_HAS_KIT) tags.unshift("kit");
      const text = u.alive
        ? `${u.hp} hp${u.armor > 0 ? ` +${u.armor}` : ""} · ${weaponName(this.rules, u.weapon)} ${u.ammo}` +
          (tags.length ? ` · ${tags.join(" ")}` : "")
        : "dead";
      row.info.replaceChildren(text);
      if (u.alive && u.state & S_HAS_CHARGE) row.info.prepend(el("span", "charge", "charge "));
    }
    this.viewButtons[0].classList.toggle("on", perspective.kind === "all");
    this.viewButtons[1].classList.toggle("on", perspective.kind === "team" && perspective.team === 0);
    this.viewButtons[2].classList.toggle("on", perspective.kind === "team" && perspective.team === 1);
    for (const [key, b] of this.layerButtons) b.classList.toggle("on", !!show[key]);
  }
}

function teamSpan(team: number, header: Header): HTMLElement {
  return el("span", teamClass(team), header.bots[team]?.name || `Team ${teamLetter(team)}`);
}

/** Ticks a plant / defuse / detonation notice stays up. */
const NOTICE_TICKS = 8;

/** The line across the top of the map: round result, match result, phase notices. */
export function updateBanner(
  root: HTMLElement,
  timeline: Timeline,
  frameIndex: number,
  frame: Frame,
  header: Header,
  map: GameMap,
): void {
  const round = timeline.rounds[roundIndexAt(timeline, frameIndex)];
  let parts: (Node | string)[] = [];
  let key = "";
  if (frame.phase === PHASE_MATCH_END) {
    key = "end";
    parts =
      frame.winner === null || frame.winner < 0
        ? [`Match over · draw ${frame.score[0]}:${frame.score[1]}`]
        : [teamSpan(frame.winner, header), ` wins the match ${frame.score[0]}:${frame.score[1]}`];
  } else if (frame.phase === PHASE_ROUND_END && round.winner !== null && round.winner >= 0) {
    key = `round${round.round}`;
    const role = round.winner === round.attackers ? "attackers" : "defenders";
    parts = [
      `Round ${round.round} · `,
      teamSpan(round.winner, header),
      ` (${role}) by ${ROUND_WIN_REASONS[round.reason] ?? "decision"}`,
    ];
  } else if (frame.phase === PHASE_DRAFT) {
    key = "draft";
    parts = ["Draft"];
  } else if (frame.phase === PHASE_BUY) {
    key = `buy${frame.round}`;
    parts = [`Round ${frame.round} · buy phase · `, teamSpan(frame.attackers, header), " attacks"];
  } else {
    const notice = recentEvents(timeline, frameIndex, NOTICE_TICKS, [EVENT_PLANT, EVENT_DEFUSE, EVENT_DETONATE])
      .filter((e) => e.frame >= round.start)
      .pop();
    if (notice) {
      key = `notice${notice.frame}:${notice.kind}`;
      parts = noticeText(notice, map);
    }
  }
  if (root.dataset.key === key) return;
  root.dataset.key = key;
  root.hidden = parts.length === 0;
  root.replaceChildren(...parts);
}

function noticeText(e: TimedEvent, map: GameMap): string[] {
  if (e.kind === EVENT_PLANT) {
    return [`Charge planted${map.sites.length > 1 ? ` at site ${siteName(e.value)}` : ""}`];
  }
  return [e.kind === EVENT_DEFUSE ? "Charge defused" : "Charge detonated"];
}

/** Round starts (coloured by who won the round) and plants on the scrub bar. Clicking the bar seeks as usual. */
export function buildMarkers(root: HTMLElement, timeline: Timeline): void {
  const last = Math.max(1, timeline.frames - 1);
  const at = (frame: number) => `${(frame / last) * 100}%`;
  root.replaceChildren();
  for (const r of timeline.rounds) {
    if (r.round <= 0) continue;
    const won = r.winner === 0 ? " won-a" : r.winner === 1 ? " won-b" : "";
    const m = el("div", `marker${won}`, el("span", "", String(r.round)));
    m.style.left = at(r.start);
    root.append(m);
    if (r.plant >= 0) {
      const p = el("div", "marker plant");
      p.style.left = at(r.plant);
      root.append(p);
    }
  }
}
