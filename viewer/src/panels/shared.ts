// What the panels share: the full ruleset as the engine exports it, how a rule field is labelled and shown,
// and the small table model the buy menu and team tab render from.
//
// Nothing here knows a weapon or an ability by name. Records are read field by field, so a field or a record
// added to `rules/*.toml` (and to the engine's export) shows up without a viewer change. The only knowledge
// kept here is how to present a field: which ones are distances, ticks, angles, percentages or credits.

import { FULL_TURN, TILE } from "../types";

/** One record of a rules table (a weapon, an ability, a kit, a shop item): `id`, `name` and its numbers. */
export interface RuleRecord {
  id: number;
  name: string;
  [field: string]: unknown;
}

/** The tables of `Engine.rawRules()`: what the buy menu and the team tab are built from. */
export interface ShopRules {
  weapons: RuleRecord[];
  abilities: RuleRecord[];
  kits: RuleRecord[];
  items: RuleRecord[];
  economy: Record<string, number>;
  core: Record<string, number>;
}

function records(raw: unknown): RuleRecord[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .filter((r): r is Record<string, unknown> => typeof r === "object" && r !== null && typeof r.id === "number")
    .map((r) => ({ ...r, id: r.id as number, name: typeof r.name === "string" && r.name ? r.name : `#${r.id}` }));
}

function numbers(raw: unknown): Record<string, number> {
  const out: Record<string, number> = {};
  if (typeof raw !== "object" || raw === null) return out;
  for (const [k, v] of Object.entries(raw)) if (typeof v === "number") out[k] = v;
  return out;
}

/** The tables of the engine's rules export, or `null` for an engine build that does not export them. */
export function parseShopRules(raw: unknown): ShopRules | null {
  if (typeof raw !== "object" || raw === null) return null;
  const r = raw as Record<string, unknown>;
  if (!Array.isArray(r.weapon_rules)) return null;
  return {
    weapons: records(r.weapon_rules),
    abilities: records(r.ability_rules),
    kits: records(r.kit_rules),
    items: records(r.item_rules),
    economy: numbers(r.economy),
    core: numbers(r.core),
  };
}

export const byId = (list: RuleRecord[], id: number): RuleRecord | undefined => list.find((r) => r.id === id);
export const nameOf = (list: RuleRecord[], id: number, none = "none"): string =>
  id < 0 ? none : (byId(list, id)?.name ?? `#${id}`);
export const num = (r: RuleRecord | undefined, field: string): number | undefined =>
  typeof r?.[field] === "number" ? (r[field] as number) : undefined;

/** `EFFECT_*` ids of `crates/abi` (an ability's `effect`, an effect's `kind` in frames). */
export const EFFECT_NAMES: Record<number, string> = {
  1: "smoke",
  2: "fire",
  3: "barrier",
  4: "flash",
  5: "recon",
  6: "heal",
  7: "decoy",
  8: "dash",
};

/** `ITEM_KIND_*` ids of `crates/abi`. */
export const ITEM_KINDS: Record<number, string> = { 1: "weapon", 2: "armour", 3: "ability charge", 4: "defuse kit" };
export const ITEM_KIND_WEAPON = 1;
export const ITEM_KIND_ARMOR = 2;
export const ITEM_KIND_ABILITY_CHARGE = 3;

// How a field is shown, by its name. Fixed point: 1024 units per tile, 65536 angle units per turn.
const DISTANCE = /(^|_)(range|radius|reach|falloff_start)$/;
const SPEED = /(^|_)speed$/;
const ANGLE = /(^|_)(spread|fov_half|turn_rate)$/;
const TICKS = /(^|_)(windup|cooldown|duration|delay|ticks)$/;
const PERCENT = /_pct$/;
const CREDITS = /(^|_)(price|reward|bonus|credits)$/;

const trim = (v: number, digits: number) => String(Number(v.toFixed(digits)));

/** A rule value with its unit, in the words a player uses (tiles, ticks, degrees, credits). */
export function formatField(field: string, value: unknown): string {
  if (typeof value === "boolean") return value ? "yes" : "no";
  if (Array.isArray(value)) return value.map((v) => formatField(field, v)).join(", ");
  if (typeof value !== "number") return value === undefined || value === null ? "" : String(value);
  if (field === "speed_pct") return `${value}%`;
  if (SPEED.test(field)) return `${trim(value / TILE, 2)} t/tick`;
  if (DISTANCE.test(field)) return `${trim(value / TILE, 1)} t`;
  if (ANGLE.test(field)) return `${trim((value / FULL_TURN) * 360, 1)}°`;
  if (TICKS.test(field)) return `${value} tk`;
  if (PERCENT.test(field)) return `${value}%`;
  if (CREDITS.test(field)) return `${value} cr`;
  return String(value);
}

/** Field name to column heading. */
export const label = (field: string): string => field.replace(/_pct$/, " %").replace(/_/g, " ");

/**
 * The fields of `rows` worth a column, in `preferred` order and then alphabetically: every field any row
 * has, so a new field in the rules gets a column. `skip` lists fields shown some other way.
 */
export function columns(rows: RuleRecord[], preferred: string[], skip: string[] = []): string[] {
  const seen = new Set<string>();
  for (const r of rows) for (const k of Object.keys(r)) seen.add(k);
  for (const k of ["id", "name", ...skip]) seen.delete(k);
  const first = preferred.filter((k) => seen.has(k));
  const rest = [...seen].filter((k) => !preferred.includes(k)).sort();
  return [...first, ...rest];
}

/** A table the panels draw: a heading row and one row of display text per record. */
export interface TableModel {
  title: string;
  note?: string;
  columns: string[];
  rows: { key: string; cells: string[]; dim?: boolean }[];
}
