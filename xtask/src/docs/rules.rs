//! `docs/generated/rules.md`: every number of the ruleset, as the engine's loader reads it.
//!
//! Values come from `ibc_engine::Ruleset::from_toml` (so defaults, clamps and derived counts are the engine's),
//! field names and meanings from the doc comments of the ABI records (`CoreRules`, `WeaponRules`, ...) and the
//! engine structs (`EconRules`, `FlowRules`), and the prose from the comments in `rules/*.toml`. A field with
//! no doc comment gets an empty Meaning cell: document the field and regenerate.

use super::md::{self, code};
use super::source::{Module, Struct};
use crate::util::Result;
use ibc_engine::Ruleset;

pub struct Inputs<'a> {
    pub ruleset: &'a Ruleset,
    pub abi: &'a Module,
    /// Engine sources that document `EconRules` and `FlowRules`.
    pub engine: &'a [Module],
    /// `(path, text)` of `rules/core.toml`, `weapons.toml`, `abilities.toml`, `economy.toml`.
    pub tomls: [(&'a str, &'a str); 4],
}

/// The fields of a ruleset record, read from its bytes at the offsets the ABI source gives.
fn values<T: Copy>(rec: &T, s: &Struct) -> Result<Vec<Vec<i32>>> {
    let bytes = ibc_abi::bytes_of(rec);
    if bytes.len() != s.size as usize {
        return Err(format!(
            "{}: the ABI source says {} bytes, the compiled record has {}",
            s.name,
            s.size,
            bytes.len()
        ));
    }
    s.fields
        .iter()
        .map(|f| {
            if f.ty != "i32" && !f.ty.starts_with("[i32;") {
                return Err(format!(
                    "{}.{}: only i32 fields are supported",
                    s.name, f.name
                ));
            }
            let start = f.offset as usize;
            let end = start + f.size.unwrap_or(0) as usize;
            Ok(bytes[start..end]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| i32::from_le_bytes(*c))
                .collect())
        })
        .collect()
}

fn show(v: &[i32]) -> String {
    match v {
        [one] => one.to_string(),
        _ => format!(
            "[{}]",
            v.iter().map(i32::to_string).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// `1 (SMOKE)` for a value with a named constant, the bare value otherwise.
fn named(abi: &Module, prefix: &str, v: i32) -> String {
    match abi.name_of(prefix, i64::from(v)) {
        Some(n) => format!("{v} ({})", &n[prefix.len()..]),
        None => v.to_string(),
    }
}

fn name_at(names: &[String], id: i32) -> String {
    usize::try_from(id)
        .ok()
        .and_then(|i| names.get(i))
        .map(|n| format!("{n} ({id})"))
        .unwrap_or_else(|| format!("none ({id})"))
}

/// One row per field, one column per entry (weapons, abilities): easier to compare than one wide row each.
fn transposed<T: Copy>(
    s: &Struct,
    names: &[String],
    recs: &[T],
    annotate: impl Fn(&str, &[i32]) -> String,
) -> Result<String> {
    let cols: Vec<Vec<Vec<i32>>> = recs.iter().map(|r| values(r, s)).collect::<Result<_>>()?;
    let mut header = vec!["Field"];
    header.extend(names.iter().map(String::as_str));
    header.push("Meaning");
    let rows: Vec<Vec<String>> = s
        .fields
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mut row = vec![code(&f.name)];
            row.extend(cols.iter().map(|c| annotate(&f.name, &c[i])));
            row.push(f.doc.clone());
            row
        })
        .collect();
    Ok(md::table(&header, &rows))
}

/// One row per entry (kits, shop items); array fields get one column per element.
fn per_entry<T: Copy>(
    s: &Struct,
    names: &[String],
    recs: &[T],
    annotate: impl Fn(&str, i32, &[Vec<i32>]) -> String,
) -> Result<String> {
    let mut header = vec!["id".to_string(), "name".to_string()];
    let mut rows = Vec::new();
    for (k, rec) in recs.iter().enumerate() {
        let vals = values(rec, s)?;
        let mut row = vec![k.to_string(), code(&names[k])];
        for (f, v) in s.fields.iter().zip(&vals).filter(|(f, _)| f.name != "id") {
            if k == 0 {
                header.extend(match v.len() {
                    1 => vec![f.name.clone()],
                    n => (0..n).map(|j| format!("{}[{j}]", f.name)).collect(),
                });
            }
            row.extend(v.iter().map(|&x| annotate(&f.name, x, &vals)));
        }
        rows.push(row);
    }
    let header: Vec<&str> = header.iter().map(String::as_str).collect();
    Ok(md::table(&header, &rows))
}

fn toml_has(text: &str, key: &str) -> bool {
    text.parse::<toml::Table>()
        .map(|t| t.contains_key(key))
        .unwrap_or(false)
}

fn core(i: &Inputs) -> Result<String> {
    let s = i.abi.record("CoreRules")?;
    let vals = values(&i.ruleset.core, s)?;
    let mut rows: Vec<Vec<String>> = s
        .fields
        .iter()
        .zip(&vals)
        .map(|(f, v)| {
            let mut doc = f.doc.clone();
            if !toml_has(i.tomls[0].1, &f.name) {
                doc = format!("{doc} *(not a key in rules/core.toml: set by the loader)*");
            }
            vec![code(&f.name), show(v), doc.trim().to_string()]
        })
        .collect();
    rows.push(vec![
        code("round_end_ticks"),
        i.ruleset.round_end_ticks.to_string(),
        "Pause after a round is decided. Not part of `T_RULES_CORE`.".to_string(),
    ]);
    rows.push(vec![
        code("stack_limit"),
        i.ruleset.stack_limit.to_string(),
        "Deepest call stack a bot may build, in stack units (params + locals + largest operand stack + 2 per \
         call). Going deeper traps. Enforced by the runner. Not part of `T_RULES_CORE`."
            .to_string(),
    ]);
    Ok(md::table(&["Key", "Value", "Meaning"], &rows))
}

fn kits(i: &Inputs) -> Result<String> {
    let r = i.ruleset;
    per_entry(
        i.abi.record("KitRules")?,
        &r.kit_names,
        &r.kits,
        |f, v, _| match f {
            "abilities" => name_at(&r.ability_names, v),
            _ => v.to_string(),
        },
    )
}

fn items(i: &Inputs) -> Result<String> {
    let r = i.ruleset;
    let s = i.abi.record("ItemRules")?;
    let kind_at = s
        .fields
        .iter()
        .position(|f| f.name == "kind")
        .ok_or("ItemRules has no `kind` field")?;
    let weapon_kind = i
        .abi
        .value_of("ITEM_KIND_WEAPON")
        .ok_or("ITEM_KIND_WEAPON not found in the ABI")?;
    per_entry(s, &r.item_names, &r.items, |f, v, all| match f {
        "kind" => named(i.abi, "ITEM_KIND_", v),
        "value" if i64::from(all[kind_at][0]) == weapon_kind => name_at(&r.weapon_names, v),
        _ => v.to_string(),
    })
}

fn armour(i: &Inputs) -> String {
    let r = i.ruleset;
    let armor = i.abi.value_of("ITEM_KIND_ARMOR");
    let rows: Vec<Vec<String>> = r
        .items
        .iter()
        .zip(&r.item_names)
        .filter(|(it, _)| Some(i64::from(it.kind)) == armor)
        .map(|(it, n)| vec![code(n), it.price.to_string(), it.value.to_string()])
        .collect();
    let mut out = md::table(&["Item", "Price", "Armour points"], &rows);
    out += &format!(
        "Armour absorbs `armor_absorb_pct` = {}% of each hit until its points are used up.\n\n",
        r.econ.armor_absorb_pct
    );
    out
}

/// `name: value` pairs of a flat struct's `{:#?}` output, in declaration order. Anything but a struct of
/// integers is an error, so a new array or nested field cannot be shown half.
fn debug_fields(dbg: &str) -> Result<Vec<(String, String)>> {
    let lines: Vec<&str> = dbg.lines().collect();
    let inner = lines
        .get(1..lines.len().saturating_sub(1))
        .unwrap_or_default();
    inner
        .iter()
        .map(|l| {
            let pair = l.trim().trim_end_matches(',').split_once(": ");
            match pair {
                Some((k, v)) if v.parse::<i64>().is_ok() => Ok((k.to_string(), v.to_string())),
                _ => Err(format!(
                    "cannot show {:?} of `{}`: only integer fields are supported (xtask/src/docs/rules.rs)",
                    l.trim(),
                    lines.first().unwrap_or(&"").trim_end_matches(" {")
                )),
            }
        })
        .collect()
}

fn economy(i: &Inputs) -> Result<String> {
    let mut rows = Vec::new();
    for (name, dbg) in [
        ("EconRules", format!("{:#?}", i.ruleset.econ)),
        ("FlowRules", format!("{:#?}", i.ruleset.flow)),
    ] {
        let s = i
            .engine
            .iter()
            .find_map(|m| m.record(name).ok())
            .ok_or_else(|| format!("struct {name} not found in the engine sources"))?;
        for (k, v) in debug_fields(&dbg)? {
            let doc = s.field(&k).map(|f| f.doc.clone()).unwrap_or_default();
            rows.push(vec![code(&k), v, doc]);
        }
    }
    Ok(md::table(&["Key", "Value", "Meaning"], &rows))
}

/// The comments of a TOML file, paragraph by paragraph. A paragraph directly above an `[[entry]]` says which.
fn notes(path: &str, text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut paras = Vec::new();
    let mut n = 0;
    while n < lines.len() {
        if !lines[n].trim_start().starts_with('#') {
            n += 1;
            continue;
        }
        let mut para = Vec::new();
        while let Some(c) = lines.get(n).and_then(|l| l.trim_start().strip_prefix('#')) {
            para.push(c.strip_prefix(' ').unwrap_or(c).trim_end());
            n += 1;
        }
        let mut text = para.join("\n");
        if let Some(header) = lines
            .get(n)
            .map(|l| l.trim())
            .filter(|l| l.starts_with("[["))
        {
            let name = lines[n + 1..]
                .iter()
                .take_while(|l| !l.trim().starts_with("[["))
                .find_map(|l| l.trim().strip_prefix("name = "))
                .unwrap_or("");
            text = format!("(above {header} {name})\n{text}");
        }
        paras.push(text);
    }
    if paras.is_empty() {
        return String::new();
    }
    format!(
        "Notes from `{path}`:\n\n```text\n{}\n```\n\n",
        paras.join("\n\n")
    )
}

pub fn render(i: &Inputs) -> Result<String> {
    let r = i.ruleset;
    let mut out =
        md::banner("`rules/*.toml` (through `ibc_engine::Ruleset`) and `crates/abi/src/lib.rs`");
    out += "# Ruleset reference\n\n";
    out += &format!(
        "Ruleset hash `{}`. Every value as the engine loads it, defaults and clamps applied. Bots read the same \
         numbers at runtime through `T_RULES_CORE`, `T_WEAPON`, `T_ABILITY`, `T_ITEM` and `T_KIT`: never \
         hard-code them. Distances are fixed-point (1024 per tile), angles 65536 per full turn, times in \
         ticks. The mechanics are in `docs/RULES.md` and `docs/rules/`.\n\n",
        ibc_replay::hash_hex(r.hash())
    );
    let [core_toml, weapons_toml, abilities_toml, economy_toml] = i.tomls;
    out += "## Core\n\n";
    out += &core(i)?;
    out += &notes(core_toml.0, core_toml.1);
    out += "## Weapons\n\n";
    out += &transposed(
        i.abi.record("WeaponRules")?,
        &r.weapon_names,
        &r.weapons,
        |_, v| show(v),
    )?;
    out += &notes(weapons_toml.0, weapons_toml.1);
    out += "## Abilities\n\n";
    let ability = i.abi.record("AbilityRules")?;
    out += &transposed(ability, &r.ability_names, &r.abilities, |f, v| {
        match (f, v) {
            ("effect", [e]) => named(i.abi, "EFFECT_", *e),
            _ => show(v),
        }
    })?;
    out += "## Kits\n\n";
    out +=
        "`abilities[0]` is the kit's signature ability, `abilities[1]` the one that is bought.\n\n";
    out += &kits(i)?;
    out += &notes(abilities_toml.0, abilities_toml.1);
    out += "## Shop\n\n";
    out += &items(i)?;
    out += "## Armour\n\n";
    out += &armour(i);
    out += "## Economy and match flow\n\n";
    out += &economy(i)?;
    out += &notes(economy_toml.0, economy_toml.1);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_keep_paragraphs_and_name_the_entry_below() {
        let text = "# Head.\n# More.\n\nkey = 1\n\n# The gun.\n[[weapon]]\nname = \"pistol\"\n";
        assert_eq!(
            notes("rules/x.toml", text),
            "Notes from `rules/x.toml`:\n\n```text\nHead.\nMore.\n\n(above [[weapon]] \"pistol\")\nThe gun.\n```\n\n"
        );
    }

    #[test]
    fn debug_output_gives_fields_in_order() {
        let f = debug_fields("X {\n    a: 1,\n    b_c: -2,\n}").unwrap();
        assert_eq!(f, [("a".into(), "1".into()), ("b_c".into(), "-2".into())]);
        assert!(debug_fields("X {\n    a: [\n        1,\n    ],\n}").is_err());
    }
}
