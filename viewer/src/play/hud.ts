// The heads-up display of the play page: phase and clock, score, health, weapon, credits, abilities, the
// engine's answers to the last acts, and the draft and buy menus. Placeholder looks; plain DOM.

import { PHASE_ACTION, PHASE_BUY, PHASE_DRAFT, PHASE_MATCH_END, PHASE_PLANTED, PHASE_ROUND_END } from "../types";
import { weaponName, type ViewRules } from "../vision";
import type { Action } from "./controls";
import type { Setup, State } from "./api";

const PHASES: Record<number, string> = {
  [PHASE_DRAFT]: "Draft",
  [PHASE_BUY]: "Buy",
  [PHASE_ACTION]: "Action",
  [PHASE_PLANTED]: "Planted",
  [PHASE_ROUND_END]: "Round over",
  [PHASE_MATCH_END]: "Match over",
};

const $ = (id: string) => document.getElementById(id)!;

function text(id: string, value: string): void {
  const el = $(id);
  if (el.textContent !== value) el.textContent = value;
}

/** Seconds (real-time) or ticks (turn mode) for a tick count. */
export function clock(ticks: number, tps: number): string {
  return tps > 0 ? `${(ticks / tps).toFixed(1)}s` : `${ticks} ticks`;
}

export function updateHud(setup: Setup, rules: ViewRules, s: State): void {
  const { me, match: m } = s.view;
  const side = m.my_side === 0 ? "attack" : "defend";
  text("hud-phase", `${PHASES[m.phase] ?? "?"} · round ${m.round} · ${clock(m.phase_ticks_left, setup.tps)}`);
  text("hud-score", `${m.score_mine} : ${m.score_theirs}  (first to ${m.rounds_to_win}) · you ${side}`);
  text("hud-alive", `allies ${m.allies_alive} · enemies ${m.enemies_alive}`);
  text("hud-hp", `HP ${me.hp}  armour ${me.armor}`);
  const reloading = me.channel > 0 ? ` (busy ${me.channel})` : "";
  text("hud-weapon", `${weaponName(rules, me.weapon)} ${me.ammo}${reloading}`);
  text("hud-credits", `credits ${me.credits}`);
  const kit = setup.kits[me.kit];
  const abil = [0, 1].map((slot) => {
    const id = kit?.abilities[slot] ?? -1;
    if (id < 0) return `${"QE"[slot]}: none`;
    const name = setup.abilities[id]?.name ?? `ability ${id}`;
    const cd = me.cooldown[slot] > 0 ? ` cd ${me.cooldown[slot]}` : "";
    return `${"QE"[slot]}: ${name} x${me.charges[slot]}${cd}`;
  });
  text("hud-kit", `${kit?.name ?? "kit ?"} · ${abil.join(" · ")}`);
  const acts = s.last_acts.filter((a) => a.act !== "move" && a.act !== "face");
  text("hud-acts", acts.map((a) => `${a.act}: ${a.result}`).join(" · "));
  text("hud-mode", s.mode === "turn" ? `turn mode · tick ${s.tick}` : `${s.running ? "running" : "paused"} · tick ${s.tick}`);
  $("draft").hidden = m.phase !== PHASE_DRAFT;
  $("buy").hidden = m.phase !== PHASE_BUY;
  const done = $("done");
  done.hidden = !s.done;
  if (s.done) text("done-text", s.done.error ?? `${s.done.result}\nreplay: ${s.done.replay}`);
}

/** Fill the draft and buy menus once; clicking an entry sends its action. */
export function buildMenus(setup: Setup, send: (a: Action) => void): void {
  const kits = $("draft-list");
  kits.replaceChildren(
    ...setup.kits.map((k, i) => {
      const names = k.abilities.map((a) => (a >= 0 ? setup.abilities[a]?.name : "-")).join(", ");
      return button(`${i + 1}. ${k.name} (hp ${k.hp}, speed ${k.speed_pct}%: ${names})`, () =>
        send({ act: "draft", kit: k.id }),
      );
    }),
  );
  const items = $("buy-list");
  items.replaceChildren(
    ...setup.items.map((it, i) => {
      const key = i < 10 ? `${(i + 1) % 10}. ` : "";
      return button(`${key}${it.name} ${it.price}`, () => send({ act: "buy", item: it.id }));
    }),
  );
}

function button(label: string, onClick: () => void): HTMLButtonElement {
  const b = document.createElement("button");
  b.textContent = label;
  b.addEventListener("click", (e) => {
    onClick();
    // Give focus back so keys keep driving the unit.
    if (e.detail > 0) b.blur();
  });
  return b;
}
