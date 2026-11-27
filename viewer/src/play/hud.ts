// The heads-up display of the play page: phase and clock, score, health, weapon, credits, abilities, the
// engine's answers to the last acts, the squad (which unit you possess) and the buy menu. Placeholder looks;
// plain DOM.

import {
  DROP_CHARGE,
  PHASE_ACTION,
  PHASE_BUY,
  PHASE_DRAFT,
  PHASE_MATCH_END,
  PHASE_PLANTED,
  PHASE_ROUND_END,
  S_ALIVE,
  SIDE_ATTACK,
} from "../types";
import { weaponName, type ViewRules } from "../vision";
import { interactHint, pickupTarget, type Action } from "./controls";
import type { PlayUnit, Setup, State } from "./api";

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

/** Seconds (on a clock) or ticks (turn mode) for a tick count. */
export function clock(ticks: number, tps: number): string {
  return tps > 0 ? `${(ticks / tps).toFixed(1)}s` : `${ticks} ticks`;
}

/** The mode line: how time passes right now. */
export function modeLine(s: State): string {
  if (s.mode === "turn") return `turn mode · tick ${s.tick}`;
  if (s.mode === "realtime") return `${s.running ? "running" : "paused"} · tick ${s.tick}`;
  return `${s.running ? "time runs (P to stop)" : "time moves while you act · hold Space to wait"} · tick ${s.tick}`;
}

export function updateHud(setup: Setup, rules: ViewRules, s: State): void {
  const { me, match: m } = s.view;
  const side = m.my_side === 0 ? "attack" : "defend";
  text("hud-phase", `${PHASES[m.phase] ?? "?"} · round ${m.round} · ${clock(m.phase_ticks_left, setup.tps)}`);
  text("hud-score", `${m.score_mine} : ${m.score_theirs}  (first to ${m.rounds_to_win}) · you ${side}`);
  text("hud-alive", `allies ${m.allies_alive} · enemies ${m.enemies_alive}`);
  if (me) {
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
    const dead = me.state & S_ALIVE ? "" : "dead: Tab to take over a living teammate · ";
    text("hud-kit", `${dead}${kit?.name ?? "kit ?"} · ${abil.join(" · ")}`);
  } else {
    text("hud-hp", "Watching");
    text("hud-weapon", "");
    text("hud-credits", "");
    text("hud-kit", "pick a unit to play (Tab, or the squad list)");
  }
  const target = me && pickupTarget(me, s.view.frame.drops, setup.core.pickup_reach, m.my_side === SIDE_ATTACK);
  const pickup = target ? (target.kind === DROP_CHARGE ? "charge" : weaponName(rules, target.item)) : null;
  const hint = interactHint(me, m, pickup);
  text("hud-interact", hint ?? "");
  $("hud-interact").hidden = !hint;
  const acts = s.last_acts.filter((a) => a.act !== "move" && a.act !== "face");
  text("hud-acts", acts.map((a) => `${a.act}: ${a.result}`).join(" · "));
  text("hud-mode", modeLine(s));
  $("buy").hidden = m.phase !== PHASE_BUY || !me;
  const done = $("done");
  done.hidden = !s.done;
  if (s.done) text("done-text", s.done.error ?? `${s.done.result}\nreplay: ${s.done.replay}`);
}

/** The next live unit of the team after the possessed one (Tab), or null if none is alive. */
export function nextAlive(setup: Setup, s: State): number | null {
  const units = s.view.frame.units as PlayUnit[];
  const start = s.possessed ?? -1;
  for (let k = 1; k <= setup.team_size; k++) {
    const slot = (start + k + setup.team_size) % setup.team_size;
    if (units[setup.first_unit + slot]?.alive) return slot;
  }
  return null;
}

/** The squad list: every unit of the team with its kit and health; clicking one possesses it. */
export function updateSquad(setup: Setup, s: State, possess: (slot: number | null) => void): void {
  const list = $("squad-list");
  const units = s.view.frame.units as PlayUnit[];
  const rows = Array.from({ length: setup.team_size }, (_, slot) => {
    const u = units[setup.first_unit + slot];
    const kit = u ? (setup.kits[u.kit]?.name ?? `kit ${u.kit}`) : "?";
    const state = !u ? "" : u.alive ? `hp ${u.hp}` : "dead";
    const mark = s.possessed === slot ? "▶ " : "";
    return { slot, label: `${mark}${slot}. ${kit} · ${state}`, alive: !!u?.alive };
  });
  const key = JSON.stringify(rows);
  if (list.dataset.key === key) return;
  list.dataset.key = key;
  list.replaceChildren(
    ...rows.map((r) => {
      const b = button(r.label, () => possess(s.possessed === r.slot ? null : r.slot));
      b.classList.toggle("active", s.possessed === r.slot);
      return b;
    }),
  );
}

/** Fill the buy menu once; clicking an entry sends its action. */
export function buildMenus(setup: Setup, send: (a: Action) => void): void {
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
