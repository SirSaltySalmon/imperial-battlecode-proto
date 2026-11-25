// The team tab (hold or tap Tab): every unit of both teams with kit, health, armour, weapons and ammo,
// credits, ability charges and score, plus the round score and the round history. A replay holds the full
// state, so this shows what neither team could see of the other during the match.

import { stateTags, unitLabel } from "../draw-units";
import { S_HAS_CHARGE, S_HAS_KIT, type Frame, type Header, type LoadoutFrame, type UnitFrame } from "../types";
import { el } from "../ui";
import { weaponName, type ViewRules } from "../vision";
import { describeRound, historyAt, teamClass, teamLetter, teamName, type RoundResult } from "./history";
import { byId, nameOf, num, type ShopRules } from "./shared";

export interface TabWeapon {
  name: string;
  ammo: number;
  /** Magazine size, when the rules say. */
  magazine?: number;
  inHand: boolean;
}

export interface TabAbility {
  name: string;
  charges: number;
  max?: number;
  cooldown: number;
}

export interface TabUnit {
  id: number;
  team: number;
  label: string;
  kit: string;
  alive: boolean;
  hp: number;
  maxHp: number;
  armor: number;
  credits: number;
  weapons: TabWeapon[];
  /** Empty when the engine build does not export loadouts. */
  abilities: TabAbility[];
  defuseKit: boolean;
  carriesCharge: boolean;
  kills: number;
  deaths: number;
  damage?: number;
  tags: string[];
}

function weapons(u: UnitFrame, lo: LoadoutFrame | undefined, shop: ShopRules | null, view: ViewRules): TabWeapon[] {
  if (!lo) return [{ name: weaponName(view, u.weapon), ammo: u.ammo, inHand: true }];
  const out: TabWeapon[] = [];
  lo.weapons.forEach((id, slot) => {
    if (id < 0) return;
    const rule = shop ? byId(shop.weapons, id) : undefined;
    out.push({ name: weaponName(view, id), ammo: lo.ammo[slot], magazine: num(rule, "magazine"), inHand: slot === lo.slot });
  });
  return out;
}

function abilities(kitId: number, lo: LoadoutFrame | undefined, shop: ShopRules | null): TabAbility[] {
  const kit = shop ? byId(shop.kits, kitId) : undefined;
  if (!lo || !shop || !Array.isArray(kit?.abilities)) return [];
  const out: TabAbility[] = [];
  (kit.abilities as unknown[]).forEach((raw, slot) => {
    const id = Number(raw);
    if (!(id >= 0)) return;
    out.push({
      name: nameOf(shop.abilities, id),
      charges: lo.charges[slot] ?? 0,
      max: num(byId(shop.abilities, id), "max_charges"),
      cooldown: lo.cooldown[slot] ?? 0,
    });
  });
  return out;
}

/** One row per unit, team A first, from a frame (and its loadouts when the engine build exports them). */
export function tabUnits(frame: Frame, shop: ShopRules | null, view: ViewRules, teamSize: number): TabUnit[] {
  return [...frame.units]
    .sort((a, b) => a.team - b.team || a.id - b.id)
    .map((u) => {
      const lo = frame.loadout?.find((l) => l.id === u.id);
      const kit = shop ? byId(shop.kits, u.kit) : undefined;
      return {
        id: u.id,
        team: u.team,
        label: unitLabel(u, teamSize),
        kit: kit?.name ?? `kit ${u.kit}`,
        alive: u.alive,
        hp: u.hp,
        maxHp: num(kit, "hp") ?? view.max_hp,
        armor: u.armor,
        credits: u.credits,
        weapons: weapons(u, lo, shop, view),
        abilities: abilities(u.kit, lo, shop),
        defuseKit: lo ? lo.has_kit : (u.state & S_HAS_KIT) !== 0,
        carriesCharge: u.alive && (u.state & S_HAS_CHARGE) !== 0,
        kills: u.kills,
        deaths: u.deaths,
        damage: lo?.damage,
        tags: u.alive ? stateTags(u) : [],
      };
    });
}

/** The text cells of a unit's row, in `TAB_COLUMNS` order. */
export function tabCells(t: TabUnit): string[] {
  const gear = [t.defuseKit ? "defuse kit" : "", t.carriesCharge ? "charge" : ""].filter(Boolean).join(", ");
  return [
    t.label,
    t.kit,
    t.alive ? (t.tags.length ? `alive · ${t.tags.join(" ")}` : "alive") : "dead",
    t.alive ? `${t.hp} / ${t.maxHp}` : "0",
    String(t.armor),
    t.weapons.map((w) => `${w.inHand ? "▸ " : ""}${w.name} ${w.ammo}${w.magazine ? `/${w.magazine}` : ""}`).join("  "),
    t.abilities
      .map((a) => `${a.name} ${a.charges}${a.max !== undefined ? `/${a.max}` : ""}${a.cooldown > 0 ? ` (cd ${a.cooldown})` : ""}`)
      .join("  ") || "-",
    gear || "-",
    String(t.credits),
    `${t.kills} / ${t.deaths}${t.damage !== undefined ? ` · ${t.damage} dmg` : ""}`,
  ];
}

export const TAB_COLUMNS = ["unit", "kit", "status", "hp", "armour", "weapons", "abilities", "gear", "credits", "K / D"];

/** The overlay. Rendered only while it is open. */
export class TeamTab {
  private header: Header | null = null;
  private shop: ShopRules | null = null;
  private view: ViewRules | null = null;
  private results: RoundResult[] = [];
  private teamSize = 1;

  constructor(private readonly root: HTMLElement) {
    root.hidden = true;
  }

  get open(): boolean {
    return !this.root.hidden;
  }

  setOpen(open: boolean): void {
    this.root.hidden = !open;
  }

  load(header: Header, shop: ShopRules | null, view: ViewRules, results: RoundResult[], teamSize: number): void {
    Object.assign(this, { header, shop, view, results, teamSize });
  }

  clear(): void {
    this.header = null;
    this.root.replaceChildren();
  }

  showEmpty(): void {
    this.root.replaceChildren(el("p", "note", "Open a replay to see the teams."));
  }

  update(frameIndex: number, frame: Frame): void {
    if (this.root.hidden) return;
    const { header, view } = this;
    if (!header || !view) {
      this.showEmpty();
      return;
    }
    const units = tabUnits(frame, this.shop, view, this.teamSize);
    const teams = [0, 1].map((team) => {
      const rows = units.filter((u) => u.team === team);
      const alive = rows.filter((u) => u.alive).length;
      const role = frame.attackers === team ? "attacking" : "defending";
      const body = rows.map((u) => el("tr", u.alive ? "" : "dim", ...tabCells(u).map((c) => el("td", "", c))));
      return el(
        "section",
        "",
        el("h3", teamClass(team), `${teamLetter(team)} · ${teamName(header, team)} · ${role} · ${alive} alive`),
        el(
          "div",
          "table-wrap",
          el("table", "", el("thead", "", el("tr", "", ...TAB_COLUMNS.map((c) => el("th", "", c)))), el("tbody", "", ...body)),
        ),
      );
    });
    const history = historyAt(this.results, frameIndex);
    const historyRows = history.map((r) =>
      el("li", teamClass(r.winner), describeRound(r)),
    );
    this.root.replaceChildren(
      el(
        "div",
        "overlay-head",
        el("h2", "", `${teamName(header, 0)} ${frame.score[0]} : ${frame.score[1]} ${teamName(header, 1)}`),
        el("span", "hint", "Tab: hold to peek, tap to pin"),
      ),
      ...teams,
      el(
        "section",
        "",
        el("h3", "", "Round history"),
        history.length ? el("ol", "history", ...historyRows) : el("p", "note", "No round decided yet."),
      ),
    );
  }
}
