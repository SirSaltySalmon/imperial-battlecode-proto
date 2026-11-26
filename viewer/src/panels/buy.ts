// The buy menu (key B): every weapon, ability, kit and shop item of the ruleset the replay was played with,
// with prices and stats. Built from the engine's rules export only; see `shared.ts` for how fields are shown.

import { el } from "../ui";
import {
  columns,
  EFFECT_NAMES,
  formatField,
  ITEM_KIND_ABILITY_CHARGE,
  ITEM_KIND_ARMOR,
  ITEM_KIND_WEAPON,
  ITEM_KINDS,
  label,
  nameOf,
  num,
  type RuleRecord,
  type ShopRules,
  type TableModel,
} from "./shared";

// Column order for the fields the ruleset has today; any other field follows alphabetically.
const WEAPON_ORDER = [
  "price",
  "slot",
  "damage",
  "pellets",
  "windup",
  "cooldown",
  "magazine",
  "reload_ticks",
  "range",
  "falloff_start",
  "falloff_min_pct",
  "bullet_speed",
  "spread",
  "move_spread",
  "recoil_spread",
  "recoil_per_shot",
  "recoil_recovery",
  "move_pct",
  "kill_reward",
];
const ABILITY_ORDER = ["price", "max_charges", "cooldown", "range", "radius", "duration", "power", "delay"];
const KIT_ORDER = ["hp", "speed_pct"];

/** Slot 0 of a kit is its signature (free) ability, slot 1 the one it buys charges of. */
const SLOT_NAMES = ["signature", "bought"];

function cell(r: RuleRecord, field: string, shop: ShopRules): string {
  const v = r[field];
  if (field === "slot" && typeof v === "number") return v === 0 ? "primary" : v === 1 ? "sidearm" : `slot ${v}`;
  if (field === "effect" && typeof v === "number") return EFFECT_NAMES[v] ?? `effect ${v}`;
  if (field === "abilities" && Array.isArray(v)) {
    return v.map((id, slot) => `${nameOf(shop.abilities, Number(id))} (${SLOT_NAMES[slot] ?? slot})`).join(", ");
  }
  return formatField(field, v);
}

function recordTable(
  title: string,
  rows: RuleRecord[],
  order: string[],
  shop: ShopRules,
  extra: { column: string; value: (r: RuleRecord) => string }[] = [],
  note?: string,
): TableModel {
  const fields = columns(rows, order);
  return {
    title,
    note,
    columns: ["name", ...fields.map(label), ...extra.map((x) => x.column)],
    rows: rows.map((r) => ({
      key: `${title}:${r.id}`,
      cells: [r.name, ...fields.map((f) => cell(r, f, shop)), ...extra.map((x) => x.value(r))],
    })),
  };
}

export function weaponTable(shop: ShopRules): TableModel {
  return recordTable(
    "Weapons",
    shop.weapons,
    WEAPON_ORDER,
    shop,
    [],
    "One trigger pull takes windup + 1 + cooldown ticks. Damage is full up to falloff start, then falls to " +
      "falloff min % at range. move % is the share of run speed kept while holding it. tk = ticks, t = tiles.",
  );
}

/** Which kits carry an ability, and in which slot. */
export function abilityKits(shop: ShopRules, ability: number): string {
  const out: string[] = [];
  for (const k of shop.kits) {
    const slots = Array.isArray(k.abilities) ? (k.abilities as unknown[]) : [];
    slots.forEach((id, slot) => {
      if (Number(id) === ability) out.push(`${k.name} (${SLOT_NAMES[slot] ?? slot})`);
    });
  }
  return out.join(", ") || "no kit";
}

export function abilityTable(shop: ShopRules): TableModel {
  return recordTable(
    "Abilities",
    shop.abilities,
    ["effect", ...ABILITY_ORDER],
    shop,
    [{ column: "kits", value: (a) => abilityKits(shop, a.id) }],
    "Price 0: refilled to max charges every round. Otherwise charges are bought (up to max charges) and kept " +
      "until used. What power means depends on the effect.",
  );
}

export function kitTable(shop: ShopRules): TableModel {
  return recordTable("Kits", shop.kits, KIT_ORDER, shop);
}

/** What a shop item gives, from its kind and value. */
function gives(item: RuleRecord, shop: ShopRules): string {
  const value = num(item, "value") ?? 0;
  switch (num(item, "kind")) {
    case ITEM_KIND_WEAPON:
      return nameOf(shop.weapons, value);
    case ITEM_KIND_ARMOR:
      return `${value} armour`;
    case ITEM_KIND_ABILITY_CHARGE:
      return `a charge of the kit's ${SLOT_NAMES[value] ?? `slot ${value}`} ability (+ its price)`;
    default:
      return ITEM_KINDS[num(item, "kind") ?? 0] ?? String(value);
  }
}

/** Armour, defuse kit, ability charges: the shop items that are not weapons (those have their own table). */
export function gearTable(shop: ShopRules): TableModel {
  const items = shop.items.filter((i) => num(i, "kind") !== ITEM_KIND_WEAPON);
  const absorb = shop.economy.armor_absorb_pct;
  return {
    title: "Armour and gear",
    note: absorb === undefined ? undefined : `Armour absorbs ${absorb}% of each hit until it is used up.`,
    columns: ["name", "kind", "price", "gives"],
    rows: items.map((i) => ({
      key: `item:${i.id}`,
      cells: [i.name, ITEM_KINDS[num(i, "kind") ?? 0] ?? String(i.kind), formatField("price", i.price), gives(i, shop)],
    })),
  };
}

/** Economy numbers as label / value pairs. */
export function economyList(shop: ShopRules): [string, string][] {
  return Object.entries(shop.economy).map(([k, v]) => [
    label(k),
    k === "default_sidearm" ? nameOf(shop.weapons, v) : formatField(k, v),
  ]);
}

export function buyTables(shop: ShopRules): TableModel[] {
  return [weaponTable(shop), abilityTable(shop), kitTable(shop), gearTable(shop)];
}

export function renderTable(t: TableModel): HTMLElement {
  const head = el("tr", "", ...t.columns.map((c) => el("th", "", c)));
  const body = t.rows.map((r) => {
    const tr = el("tr", r.dim ? "dim" : "", ...r.cells.map((c) => el("td", "", c)));
    tr.dataset.key = r.key;
    return tr;
  });
  return el(
    "section",
    "",
    el("h3", "", t.title),
    el("div", "table-wrap", el("table", "", el("thead", "", head), el("tbody", "", ...body))),
    ...(t.note ? [el("p", "note", t.note)] : []),
  );
}

/** The overlay. Rebuilt only when the ruleset changes (a replay from another engine build is opened). */
export class BuyMenu {
  constructor(private readonly root: HTMLElement) {
    root.hidden = true;
  }

  /** `loaded`: a replay is open (so a null `shop` means its engine build exports no rule tables). */
  setRules(shop: ShopRules | null, loaded: boolean): void {
    const head = el("div", "overlay-head", el("h2", "", "Buy menu"), el("span", "hint", "B or Esc to close"));
    if (!shop) {
      const text = loaded
        ? "The engine build that plays this replay does not export its rule tables."
        : "Open a replay: the buy menu shows the ruleset of the engine build that plays it.";
      this.root.replaceChildren(head, el("p", "note", text));
      return;
    }
    const econ = el("dl", "econ");
    for (const [k, v] of economyList(shop)) econ.append(el("dt", "", k), el("dd", "", v));
    this.root.replaceChildren(
      head,
      ...buyTables(shop).map(renderTable),
      el("section", "", el("h3", "", "Economy"), econ),
    );
  }

  toggle(): void {
    this.root.hidden = !this.root.hidden;
  }

  close(): void {
    this.root.hidden = true;
  }
}
