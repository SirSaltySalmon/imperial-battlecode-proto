//! `docs/generated/abi.md`: every constant and every `repr(C)` record of `crates/abi/src/lib.rs`.

use super::md::{self, cell, code};
use super::source::{Const, Module, Struct};

/// How constants are grouped on the page. An entry ending in `_` is a name prefix, anything else an exact
/// name; the first group that matches wins. Constants no group claims are listed under "Other constants", so
/// a new group of ids is never left out, only left unnamed.
const GROUPS: &[(&str, &[&str], &str)] = &[
    (
        "General",
        &["ABI_VERSION", "IMPORT_MODULE", "TILE", "FULL_TURN"],
        "",
    ),
    (
        "Error codes",
        &["OK", "ERR_"],
        "Returned by `act`, `mem_read` and `mem_write` as is, and by `count` and `get` negated.",
    ),
    (
        "Tables",
        &["T_"],
        "The `table` argument of `count` and `get`.",
    ),
    (
        "Actions",
        &["A_"],
        "The `kind` argument of `act(kind, a, b, c)`.",
    ),
    (
        "Memory scopes",
        &["MEM_"],
        "The `scope` argument of `mem_read` and `mem_write`.",
    ),
    ("Teams", &["TEAM_"], "`UnitInfo.team`, `MatchInfo.my_team`."),
    ("Sides", &["SIDE_"], "`UnitInfo.side`, `MatchInfo.my_side`."),
    (
        "Map tiles",
        &["TILE_"],
        "One byte per tile in a `T_MAP_ROW` row.",
    ),
    ("Phases", &["PHASE_"], "`MatchInfo.phase`."),
    ("Unit state flags", &["S_"], "Bits of `UnitInfo.state`."),
    ("Sounds", &["SOUND_"], "`SoundInfo.kind`."),
    (
        "Effects",
        &["EFFECT_"],
        "`EffectInfo.kind` and `AbilityRules.effect`.",
    ),
    (
        "Drops",
        &["DROP_"],
        "`DropInfo.kind` and the argument of `A_DROP`.",
    ),
    ("Events", &["EVENT_"], "`EventInfo.kind`."),
    ("Round win reasons", &["ROUND_WIN_"], ""),
    (
        "Debug drawing",
        &["DEBUG_"],
        "The `kind` argument of `debug_draw`.",
    ),
    ("Shop item kinds", &["ITEM_KIND_"], "`ItemRules.kind`."),
    (
        "Fuel per host call",
        &["FUEL_"],
        "Charged on top of the WebAssembly instructions the call itself runs.",
    ),
];

fn matches(name: &str, pattern: &str) -> bool {
    if pattern.ends_with('_') {
        name.starts_with(pattern)
    } else {
        name == pattern
    }
}

/// The index into `GROUPS` of a constant, or `GROUPS.len()` for "Other constants".
fn group_of(name: &str) -> usize {
    GROUPS
        .iter()
        .position(|(_, patterns, _)| patterns.iter().any(|p| matches(name, p)))
        .unwrap_or(GROUPS.len())
}

fn value_cell(c: &Const) -> String {
    match c.value {
        Some(v) if c.expr != v.to_string() => format!("`{}` = {v}", c.expr),
        _ => code(&c.expr),
    }
}

fn constants(abi: &Module) -> String {
    let mut out = String::from("## Constants\n\n");
    for g in 0..=GROUPS.len() {
        let members: Vec<&Const> = abi
            .consts
            .iter()
            .filter(|c| group_of(&c.name) == g)
            .collect();
        if members.is_empty() {
            continue;
        }
        let (title, _, blurb) = GROUPS
            .get(g)
            .copied()
            .unwrap_or(("Other constants", &[], ""));
        out += &format!("### {title}\n\n");
        if !blurb.is_empty() {
            out += &format!("{blurb}\n\n");
        }
        let rows: Vec<Vec<String>> = members
            .iter()
            .map(|c| vec![code(&c.name), value_cell(c), code(&c.ty), c.doc.clone()])
            .collect();
        out += &md::table(&["Name", "Value", "Type", "Meaning"], &rows);
    }
    out
}

fn record(s: &Struct) -> String {
    let mut out = format!("### {}\n\n", s.name);
    if !s.doc.is_empty() {
        out += &format!("{}\n\n", cell(&s.doc));
    }
    out += &format!("{} bytes, {} fields.\n\n", s.size, s.fields.len());
    let rows: Vec<Vec<String>> = s
        .fields
        .iter()
        .map(|f| {
            vec![
                f.offset.to_string(),
                code(&f.name),
                code(&f.ty),
                f.size.map(|n| n.to_string()).unwrap_or_default(),
                f.doc.clone(),
            ]
        })
        .collect();
    out + &md::table(&["Offset", "Field", "Type", "Bytes", "Meaning"], &rows)
}

pub fn render(abi: &Module) -> String {
    let mut out = md::banner("`crates/abi/src/lib.rs`");
    out += "# Bot ABI reference\n\n";
    out += "Every constant and every `repr(C)` record of the ABI, read from its source; the host imports are \
            listed in the overview. The conventions (tables, \
            intents, fog, fuel) are explained in `docs/ABI.md`; words are defined in `docs/GLOSSARY.md`.\n\n";
    out += "## Overview\n\n";
    out += &abi.doc;
    out += "\n\n";
    out += &constants(abi);
    out += "## Records\n\n";
    out += "Rows returned by `get`: `#[repr(C)]`, little-endian, fields at the offsets below. A record only \
            ever grows at the end, so a bot built against an older ABI reads the prefix it knows.\n\n";
    for s in abi.structs.iter().filter(|s| s.repr_c) {
        out += &record(s);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_are_grouped_and_unknown_ones_are_still_listed() {
        assert_eq!(group_of("TILE"), 0);
        assert_eq!(
            group_of("TILE_WALL"),
            GROUPS.iter().position(|g| g.0 == "Map tiles").unwrap()
        );
        assert_eq!(
            group_of("SOUND_STEP"),
            GROUPS.iter().position(|g| g.0 == "Sounds").unwrap()
        );
        let m = Module::parse("/// New.\npub const NEW_THING: i32 = 3;", "test").unwrap();
        let page = constants(&m);
        assert!(
            page.contains("### Other constants")
                && page.contains("| `NEW_THING` | `3` | `i32` | New. |")
        );
    }

    #[test]
    fn parsed_layout_equals_the_compiled_one() {
        use ibc_abi::*;
        use std::mem::{offset_of, size_of};
        let abi = super::super::tests::abi();
        let sizes = [
            ("UnitInfo", size_of::<UnitInfo>()),
            ("SoundInfo", size_of::<SoundInfo>()),
            ("ProjectileInfo", size_of::<ProjectileInfo>()),
            ("EffectInfo", size_of::<EffectInfo>()),
            ("MatchInfo", size_of::<MatchInfo>()),
            ("MapInfo", size_of::<MapInfo>()),
            ("SiteInfo", size_of::<SiteInfo>()),
            ("DropInfo", size_of::<DropInfo>()),
            ("EventInfo", size_of::<EventInfo>()),
            ("CoreRules", size_of::<CoreRules>()),
            ("WeaponRules", size_of::<WeaponRules>()),
            ("AbilityRules", size_of::<AbilityRules>()),
            ("ItemRules", size_of::<ItemRules>()),
            ("KitRules", size_of::<KitRules>()),
        ];
        for (name, size) in sizes {
            assert_eq!(abi.record(name).unwrap().size as usize, size, "{name}");
        }
        let unit = abi.record("UnitInfo").unwrap();
        assert_eq!(
            unit.field("charges").unwrap().offset as usize,
            offset_of!(UnitInfo, charges)
        );
        assert_eq!(
            unit.field("blind").unwrap().offset as usize,
            offset_of!(UnitInfo, blind)
        );
        let kit = abi.record("KitRules").unwrap();
        assert_eq!(
            kit.field("hp").unwrap().offset as usize,
            offset_of!(KitRules, hp)
        );
    }
}
