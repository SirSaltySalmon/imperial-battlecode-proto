import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { abilityKits, abilityTable, buyTables, economyList, gearTable, kitTable, weaponTable } from "../src/panels/buy";
import { columns, formatField, parseShopRules, type ShopRules } from "../src/panels/shared";

/** The engine's rules export (`ibc_rules_ptr`), as `crates/engine-wasm/src/view.rs` writes it. */
const RAW = JSON.parse(readFileSync(new URL("./fixtures/rules.json", import.meta.url), "utf8"));

function shop(edit: (raw: typeof RAW) => void = () => {}): ShopRules {
  const raw = structuredClone(RAW);
  edit(raw);
  return parseShopRules(raw)!;
}

describe("buy menu", () => {
  it("lists every weapon with its price and stats", () => {
    const t = weaponTable(shop());
    expect(t.rows.map((r) => r.cells[0])).toEqual(RAW.weapon_rules.map((w: { name: string }) => w.name));
    expect(t.columns.slice(0, 4)).toEqual(["name", "price", "slot", "damage"]);
    const rifle = t.rows[1].cells;
    expect(rifle[t.columns.indexOf("price")]).toBe("2700 cr");
    expect(rifle[t.columns.indexOf("slot")]).toBe("primary");
    expect(rifle[t.columns.indexOf("magazine")]).toBe("8");
    expect(rifle[t.columns.indexOf("range")]).toBe("30 t");
    expect(rifle[t.columns.indexOf("reload ticks")]).toBe("4 tk");
    expect(rifle[t.columns.indexOf("falloff min %")]).toBe("70%");
  });

  it("a new weapon in the rules is a new row, a new field a new column", () => {
    const base = weaponTable(shop());
    const t = weaponTable(
      shop((raw) => raw.weapon_rules.push({ id: 99, name: "railgun", price: 4100, damage: 90, heat: 7 })),
    );
    expect(t.rows).toHaveLength(base.rows.length + 1);
    const rail = t.rows[t.rows.length - 1].cells;
    expect(rail[0]).toBe("railgun");
    expect(t.columns).toContain("heat");
    expect(rail[t.columns.indexOf("heat")]).toBe("7");
    expect(rail[t.columns.indexOf("price")]).toBe("4100 cr");
    // Fields the new weapon lacks are blank, not invented.
    expect(rail[t.columns.indexOf("magazine")]).toBe("");
  });

  it("abilities show price, charges and which kits carry them in which slot", () => {
    const s = shop();
    const t = abilityTable(s);
    expect(t.rows).toHaveLength(RAW.ability_rules.length);
    const flash = t.rows.find((r) => r.cells[0] === "flash")!.cells;
    expect(flash[t.columns.indexOf("effect")]).toBe("flash");
    expect(flash[t.columns.indexOf("price")]).toBe("250 cr");
    expect(flash[t.columns.indexOf("max charges")]).toBe("2");
    expect(flash[t.columns.indexOf("kits")]).toBe("lancer (bought)");
    expect(abilityKits(s, 3)).toBe("seeker (signature), bulwark (signature)");
    expect(abilityKits(s, 42)).toBe("no kit");
  });

  it("kits, armour and the economy", () => {
    const s = shop();
    const kits = kitTable(s);
    const lancer = kits.rows.find((r) => r.cells[0] === "lancer")!.cells;
    expect(lancer[kits.columns.indexOf("abilities")]).toBe("dash (signature), flash (bought)");
    expect(lancer[kits.columns.indexOf("speed %")]).toBe("105%");
    const gear = gearTable(s);
    expect(gear.rows.map((r) => r.cells[0])).toEqual([
      "light_armor",
      "heavy_armor",
      "defuse_kit",
      "ability_charge_0",
      "ability_charge_1",
    ]);
    expect(gear.rows[1].cells).toEqual(["heavy_armor", "armour", "1000 cr", "50 armour"]);
    expect(gear.note).toContain("50%");
    expect(economyList(s)).toContainEqual(["default sidearm", "pistol"]);
    expect(economyList(s)).toContainEqual(["start credits", "800 cr"]);
    expect(buyTables(s).map((t) => t.title)).toEqual(["Weapons", "Abilities", "Kits", "Armour and gear"]);
  });

  it("an engine build without the tables has no buy menu", () => {
    expect(parseShopRules({ max_hp: 100, weapons: ["pistol"] })).toBeNull();
    expect(parseShopRules(null)).toBeNull();
  });

  it("formats fields by unit and orders columns", () => {
    expect(formatField("bullet_speed", 14336)).toBe("14 t/tick");
    expect(formatField("spread", 16384)).toBe("90°");
    expect(formatField("cooldown", 0)).toBe("0 tk");
    expect(formatField("pellets", 3)).toBe("3");
    expect(columns([{ id: 0, name: "x", b: 1, a: 2, price: 3 }], ["price"])).toEqual(["price", "a", "b"]);
  });
});
