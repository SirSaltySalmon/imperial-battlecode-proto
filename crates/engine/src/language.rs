//! Per-language allowances: the `[[language]]` rows of `rules/core.toml`. Each row scales a bot's fuel budget
//! and its memory cap by its language. The engine itself never meters bots; the runner and the coordinator's
//! upload check read these rows from the ruleset, so local runs and the ladder apply the same numbers.

use toml::Table;

/// The row a language without its own row gets, and the name of the row that overrides it.
pub const DEFAULT_LANGUAGE: &str = "default";
/// Bytes of one wasm page: memory caps are whole pages.
pub const WASM_PAGE: u64 = 1 << 16;
/// Largest multiplier, in thousandths (x1000), and the smallest (x0.001). The runner's fuel range.
pub const MAX_MILLI: i32 = 1_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanguageRules {
    /// The name a module gives in its `ibc.language` section, or `default`.
    pub name: String,
    /// Multiplier on the per-tick fuel budget, in thousandths (1000 = x1).
    pub fuel_milli: i32,
    /// Multiplier on `memory_bytes`, in thousandths (1000 = x1).
    pub memory_milli: i32,
}

impl LanguageRules {
    /// The `default` row of a ruleset that does not set one: x1 for both.
    pub fn unit() -> LanguageRules {
        LanguageRules { name: DEFAULT_LANGUAGE.into(), fuel_milli: 1000, memory_milli: 1000 }
    }

    /// The memory cap of a bot of this language: `memory_bytes` scaled, rounded down to whole wasm pages,
    /// at most the 4 GiB a 32-bit memory can address.
    pub fn memory_cap(&self, memory_bytes: i32) -> u64 {
        let scaled = memory_bytes.max(0) as u64 * self.memory_milli.max(0) as u64 / 1000;
        (scaled / WASM_PAGE * WASM_PAGE).min(WASM_PAGE << 16)
    }
}

fn name_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// The `[[language]]` rows of the core table, sorted by name so their order in the file does not matter.
pub fn parse(core: &Table) -> Result<Vec<LanguageRules>, String> {
    let rows = match core.get("language") {
        None => return Ok(Vec::new()),
        Some(v) => v.as_array().ok_or("core: `language` must be an array of [[language]] tables")?,
    };
    let mut out: Vec<LanguageRules> = Vec::with_capacity(rows.len());
    for row in rows {
        let t = row.as_table().ok_or("core: every [[language]] must be a table")?;
        let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if !name_ok(name) {
            return Err(format!("core: [[language]] name {name:?} must be 1 to 32 of a-z, 0-9 and _"));
        }
        let milli = |key: &str| -> Result<i32, String> {
            let v = t.get(key).and_then(|v| v.as_integer()).unwrap_or(1000);
            if !(1..=MAX_MILLI as i64).contains(&v) {
                return Err(format!("core: [[language]] {name}: {key} {v} is out of range (1 ..= {MAX_MILLI})"));
            }
            Ok(v as i32)
        };
        out.push(LanguageRules { name: name.into(), fuel_milli: milli("fuel_milli")?, memory_milli: milli("memory_milli")? });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(w) = out.windows(2).find(|w| w[0].name == w[1].name) {
        return Err(format!("core: [[language]] {} appears twice", w[0].name));
    }
    Ok(out)
}

/// The row of `language` among `rows`: its own, else the `default` row, else x1.
pub fn lookup(rows: &[LanguageRules], language: &str) -> LanguageRules {
    let find = |n: &str| rows.iter().find(|r| r.name == n).cloned();
    find(language).or_else(|| find(DEFAULT_LANGUAGE)).unwrap_or_else(LanguageRules::unit)
}

/// Adds the rows to a ruleset hash. A ruleset without rows adds nothing, so rulesets written before the table
/// existed (old replays) keep their hash.
pub fn hash(rows: &[LanguageRules], h: &mut crate::Fnv) {
    if rows.is_empty() {
        return;
    }
    h.u64(rows.len() as u64);
    for r in rows {
        h.bytes(r.name.as_bytes());
        h.i32(r.fuel_milli);
        h.i32(r.memory_milli);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str) -> Result<Vec<LanguageRules>, String> {
        parse(&text.parse::<Table>().unwrap())
    }

    #[test]
    fn rows_are_sorted_and_looked_up_with_a_default() {
        let r = rows(
            "[[language]]\nname = \"python\"\nfuel_milli = 20000\nmemory_milli = 4000\n\
             [[language]]\nname = \"default\"\nfuel_milli = 500\n",
        )
        .unwrap();
        assert_eq!(r[0].name, "default");
        assert_eq!(lookup(&r, "python").fuel_milli, 20000);
        assert_eq!(lookup(&r, "zig"), LanguageRules { name: "default".into(), fuel_milli: 500, memory_milli: 1000 });
        assert_eq!(lookup(&[], "rust"), LanguageRules::unit());
    }

    #[test]
    fn bad_rows_are_refused() {
        assert!(rows("[[language]]\nname = \"Rust\"\n").is_err());
        assert!(rows("[[language]]\nname = \"rust\"\nfuel_milli = 0\n").is_err());
        assert!(rows("[[language]]\nname = \"rust\"\n[[language]]\nname = \"rust\"\n").is_err());
        assert!(rows("language = 3\n").is_err());
    }

    #[test]
    fn the_memory_cap_is_whole_pages() {
        let mut l = LanguageRules::unit();
        assert_eq!(l.memory_cap(8 << 20), 8 << 20);
        l.memory_milli = 1500;
        assert_eq!(l.memory_cap(8 << 20), 12 << 20);
        l.memory_milli = 1001;
        assert_eq!(l.memory_cap(8 << 20), 8 << 20); // 8 MiB + 8 KiB rounds down to 128 pages
        l.memory_milli = MAX_MILLI;
        assert_eq!(l.memory_cap(i32::MAX), 1 << 32);
    }

    #[test]
    fn no_rows_hash_to_nothing() {
        let (mut a, b) = (crate::Fnv::new(), crate::Fnv::new());
        hash(&[], &mut a);
        assert_eq!(a.finish(), b.finish());
        hash(&[LanguageRules::unit()], &mut a);
        assert_ne!(a.finish(), b.finish());
    }
}
