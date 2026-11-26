//! `docs/generated/llm.md`: one Markdown file with everything a bot author (or their AI agent) needs, in
//! the spirit of Cambridge Battlecode's "Download for LLM". It is a concatenation of the pages below plus a
//! quick reference of the Rust SDK read from its doc comments.

use super::md;

/// A declaration that may span several lines, on one line: everything before its body (`{`) or its end
/// (`;`) outside brackets, so `-> [i32; N]` is kept whole.
fn signature(lines: &[&str]) -> String {
    let (mut sig, mut depth) = (String::new(), 0i32);
    'lines: for l in lines {
        sig.push(' ');
        for ch in l.trim().chars() {
            match ch {
                '(' | '[' => depth += 1,
                ')' | ']' => depth -= 1,
                '{' | ';' if depth == 0 => break 'lines,
                _ => {}
            }
            sig.push(ch);
        }
    }
    sig.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("( ", "(")
        .replace(", )", ")")
        .replace(" )", ")")
}

/// A public top-level declaration; a macro counts when it is `#[macro_export]`ed.
fn is_declaration(t: &str, exported: bool) -> bool {
    [
        "pub fn ",
        "pub struct ",
        "pub trait ",
        "pub enum ",
        "pub const ",
    ]
    .iter()
    .any(|p| t.starts_with(p))
        || (exported && t.starts_with("macro_rules! "))
}

/// The type an `impl` line is for: `impl Grid {` gives `Grid`; trait impls give `None` (their methods are
/// documented on the trait).
fn impl_target(t: &str) -> Option<String> {
    let rest = t.strip_prefix("impl")?.trim_start();
    let rest = if rest.starts_with('<') {
        rest.split_once('>')?.1.trim_start()
    } else {
        rest
    };
    if rest.contains(" for ") {
        return None;
    }
    let name = rest.split(['{', '<', ' ']).next()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The trait a `pub trait` line declares: `pub trait UnitExt {` gives `UnitExt`.
fn trait_name(t: &str) -> Option<String> {
    let rest = t.strip_prefix("pub trait ")?;
    let name = rest.split(['{', '<', ':', ' ']).next()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Public items of one SDK source file: top-level declarations, the public methods of inherent impls and
/// the methods of public traits, each with its `///` comment. Line based on purpose: rustfmt keeps the SDK
/// in this shape.
fn items(src: &str) -> Vec<String> {
    let lines: Vec<&str> = src.lines().collect();
    let (mut out, mut doc, mut owner) = (Vec::new(), Vec::<&str>::new(), None::<String>);
    let (mut exported, mut in_trait) = (false, false);
    for (n, line) in lines.iter().enumerate() {
        let t = line.trim();
        let indent = line.len() - line.trim_start().len();
        if let Some(d) = t.strip_prefix("///") {
            doc.push(d.strip_prefix(' ').unwrap_or(d));
            continue;
        }
        if t.starts_with("#[") {
            exported |= t == "#[macro_export]";
            continue;
        }
        if indent == 0 && t.starts_with("impl") {
            (owner, in_trait) = (impl_target(t), false);
        } else if indent == 0 && t.starts_with("pub trait ") {
            (owner, in_trait) = (trait_name(t), true);
        } else if indent == 0 && t == "}" {
            (owner, in_trait) = (None, false);
        }
        let method_prefix = if in_trait { "fn " } else { "pub fn " };
        let method = indent == 4 && owner.is_some() && t.starts_with(method_prefix);
        if (indent == 0 && is_declaration(t, exported)) || method {
            let mut entry = format!("- `{}`", signature(&lines[n..]));
            if method {
                entry += &format!(" (`{}`)", owner.as_deref().unwrap_or(""));
            }
            if !doc.is_empty() {
                entry += &format!(": {}", md::cell(&doc.join("\n")));
            }
            out.push(entry);
        }
        doc.clear();
        exported = false;
    }
    out
}

fn module_doc(src: &str) -> String {
    let lines: Vec<&str> = src
        .lines()
        .map_while(|l| l.strip_prefix("//!"))
        .map(|l| l.strip_prefix(' ').unwrap_or(l))
        .collect();
    lines.join("\n").trim().to_string()
}

/// The quick reference of the Rust SDK: per file, its module comment and its public items.
pub fn sdk_reference(files: &[(&str, &str)]) -> String {
    let mut out = String::from("# Rust SDK quick reference\n\n");
    out += "Public items of the `ibc` crate (`sdk/rust`), read from the source. Every ABI constant and record \
            is re-exported at the crate root as well.\n\n";
    for (path, src) in files {
        out += &format!("## `{path}`\n\n");
        let doc = module_doc(src);
        if !doc.is_empty() {
            out += &format!("{doc}\n\n");
        }
        for item in items(src) {
            out += &item;
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

/// The bundle: a table of contents, then each `(source, text)` part in order, separated by rules.
pub fn render(parts: &[(&str, &str)]) -> String {
    let mut out = md::banner("the pages it lists");
    out += "# Imperial Battlecode: everything in one file\n\n";
    out += "For AI agents and for reading offline. Use the words exactly as `docs/GLOSSARY.md` defines them. \
            Links inside the parts are relative to the file each part came from.\n\n";
    for (n, (source, _)) in parts.iter().enumerate() {
        out += &format!("{}. `{source}`\n", n + 1);
    }
    for (source, text) in parts {
        out += &format!("\n---\n\n<!-- part: {source} -->\n\n");
        out += text.trim_end();
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_items_have_signatures_docs_and_owners() {
        let src = "//! Top.\n\n/// Own unit.\npub fn me() -> UnitInfo {\n    x\n}\n\nimpl Grid {\n    \
                   /// Load it.\n    pub fn load(\n        a: i32,\n    ) -> Grid {\n    }\n    fn private() {}\n}\n\n\
                   impl Bot for X {\n    pub fn not_listed() {}\n}\n\
                   pub fn get<const N: usize>() -> [i32; N] {}\n\
                   pub trait Bot {\n    fn new() -> Self\n    where\n        Self: Sized;\n    \
                   /// Once per tick.\n    fn turn(&mut self);\n}\n";
        assert_eq!(module_doc(src), "Top.");
        assert_eq!(
            items(src),
            [
                "- `pub fn me() -> UnitInfo`: Own unit.",
                "- `pub fn load(a: i32) -> Grid` (`Grid`): Load it.",
                "- `pub fn get<const N: usize>() -> [i32; N]`",
                "- `pub trait Bot`",
                "- `fn new() -> Self where Self: Sized` (`Bot`)",
                "- `fn turn(&mut self)` (`Bot`): Once per tick.",
            ]
        );
    }
}
