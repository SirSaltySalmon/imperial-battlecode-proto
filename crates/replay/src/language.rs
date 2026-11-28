//! What a bot module says about the language it was built from. The runner gives a bot the ruleset's
//! `[[language]]` row of that language, and the coordinator's upload check reads it the same way, so a module
//! gets the same allowances in a local run and on the ladder.

use std::collections::BTreeSet;
use wasmparser::{KnownCustom, Parser, Payload};

/// Custom section in which a toolchain names the language of a module, as UTF-8 text (`rust`, `cpp`,
/// `python`). When present it takes precedence over the `producers` section.
pub const LANGUAGE_SECTION: &str = "ibc.language";

/// The ruleset's name for a language or compiler named in a `producers` section, if it knows it.
fn producer_language(field: &str, name: &str) -> Option<&'static str> {
    let name = name.to_ascii_lowercase();
    // C and C++ share the C++ toolchain (`ibc build` of a C++ bot): C99, C11, C17, C++14, ...
    let c_family = name == "c"
        || name.starts_with("c++")
        || name.starts_with("c_plus_plus")
        || (name.len() > 1
            && name.starts_with('c')
            && name[1..].bytes().all(|b| b.is_ascii_digit()));
    match field {
        "language" if name == "rust" => Some("rust"),
        "language" if c_family => Some("cpp"),
        "processed-by" if name == "rustc" => Some("rust"),
        "processed-by" if name == "clang" => Some("cpp"),
        _ => None,
    }
}

/// The languages a module says it was built from: the `ibc.language` custom section if it has one,
/// otherwise the languages and compilers of its `producers` section (WebAssembly tool conventions), in the
/// ruleset's names. Empty when the module says nothing (release builds often strip both). This catches
/// mistakes; it proves nothing, since a module can be stripped or carry any section.
pub fn detect_languages(wasm: &[u8]) -> BTreeSet<String> {
    let mut declared = BTreeSet::new();
    let mut produced = BTreeSet::new();
    for payload in Parser::new(0).parse_all(wasm) {
        let Ok(Payload::CustomSection(section)) = payload else {
            continue;
        };
        if section.name() == LANGUAGE_SECTION {
            if let Ok(text) = std::str::from_utf8(section.data()) {
                declared.insert(text.trim().to_ascii_lowercase());
            }
            continue;
        }
        let KnownCustom::Producers(reader) = section.as_known() else {
            continue;
        };
        for field in reader.into_iter().flatten() {
            for value in field.values.into_iter().flatten() {
                if let Some(lang) = producer_language(field.name, value.name) {
                    produced.insert(lang.to_string());
                }
            }
        }
    }
    if declared.is_empty() {
        produced
    } else {
        declared
    }
}
