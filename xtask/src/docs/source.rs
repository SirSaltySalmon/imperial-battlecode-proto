//! Rust source read as syntax (`syn`): constants and structs with their doc comments, and the `repr(C)`
//! layout of records made of fixed-size integers. Nothing here is hand-kept: a new id or field in the source
//! shows up in the generated pages on the next `cargo xtask docs`.

use crate::util::Result;
use syn::{AttrStyle, Attribute, BinOp, Expr, Fields, Item, Lit, Meta, Type, UnOp};

pub struct Const {
    pub name: String,
    pub ty: String,
    /// The expression as written, e.g. `1 << 3`, `100_000` is written as `100000`.
    pub expr: String,
    /// The value, when the expression is an integer.
    pub value: Option<i64>,
    pub doc: String,
}

pub struct Field {
    pub name: String,
    pub ty: String,
    pub doc: String,
    /// Byte offset in a `repr(C)` record; 0 for other structs.
    pub offset: u32,
    /// Size in bytes, when the type has a fixed layout (integers and arrays of them).
    pub size: Option<u32>,
}

pub struct Struct {
    pub name: String,
    pub doc: String,
    pub repr_c: bool,
    pub fields: Vec<Field>,
    /// Total size of a `repr(C)` record, including trailing padding; 0 for other structs.
    pub size: u32,
}

pub struct Module {
    /// The `//!` comment at the top of the file.
    pub doc: String,
    pub consts: Vec<Const>,
    pub structs: Vec<Struct>,
}

impl Module {
    pub fn parse(src: &str, what: &str) -> Result<Module> {
        let file = syn::parse_file(src).map_err(|e| format!("{what}: {e}"))?;
        let mut m = Module {
            doc: docs(&file.attrs, true),
            consts: Vec::new(),
            structs: Vec::new(),
        };
        for item in &file.items {
            match item {
                Item::Const(c) => m.consts.push(Const {
                    name: c.ident.to_string(),
                    ty: type_name(&c.ty),
                    expr: expr_text(&c.expr).ok_or_else(|| {
                        format!(
                            "{what}: constant {} has an expression docs cannot show",
                            c.ident
                        )
                    })?,
                    value: eval(&c.expr),
                    doc: docs(&c.attrs, false),
                }),
                Item::Struct(s) => m.structs.push(parse_struct(s, what)?),
                _ => {}
            }
        }
        Ok(m)
    }

    pub fn record(&self, name: &str) -> Result<&Struct> {
        self.structs
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| format!("struct {name} not found"))
    }

    /// The constant named `<prefix><something>` whose value is `value`, e.g. `EFFECT_SMOKE` for 1.
    pub fn name_of(&self, prefix: &str, value: i64) -> Option<&str> {
        self.consts
            .iter()
            .find(|c| c.name.starts_with(prefix) && c.value == Some(value))
            .map(|c| c.name.as_str())
    }

    pub fn value_of(&self, name: &str) -> Option<i64> {
        self.consts
            .iter()
            .find(|c| c.name == name)
            .and_then(|c| c.value)
    }
}

impl Struct {
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }
}

fn parse_struct(s: &syn::ItemStruct, what: &str) -> Result<Struct> {
    let repr_c = s.attrs.iter().any(|a| match &a.meta {
        Meta::List(l) => {
            l.path.is_ident("repr") && l.tokens.to_string().split(',').any(|r| r.trim() == "C")
        }
        _ => false,
    });
    let mut out = Struct {
        name: s.ident.to_string(),
        doc: docs(&s.attrs, false),
        repr_c,
        fields: Vec::new(),
        size: 0,
    };
    let Fields::Named(named) = &s.fields else {
        return Ok(out);
    };
    let (mut offset, mut max_align) = (0u32, 1u32);
    for f in &named.named {
        let layout = layout(&f.ty);
        let mut field = Field {
            name: f.ident.as_ref().map(|i| i.to_string()).unwrap_or_default(),
            ty: type_name(&f.ty),
            doc: docs(&f.attrs, false),
            offset: 0,
            size: layout.map(|(size, _)| size),
        };
        if repr_c {
            let (size, align) = layout.ok_or_else(|| {
                format!("{what}: {}.{} has no fixed layout", out.name, field.name)
            })?;
            offset = offset.next_multiple_of(align);
            field.offset = offset;
            offset += size;
            max_align = max_align.max(align);
        }
        out.fields.push(field);
    }
    if repr_c {
        out.size = offset.next_multiple_of(max_align);
    }
    Ok(out)
}

/// The text of `///` (outer) or `//!` (inner) doc comments, one leading space removed per line.
fn docs(attrs: &[Attribute], inner: bool) -> String {
    let lines: Vec<String> = attrs
        .iter()
        .filter(|a| matches!(a.style, AttrStyle::Inner(_)) == inner)
        .filter_map(|a| match &a.meta {
            Meta::NameValue(nv) if nv.path.is_ident("doc") => match &nv.value {
                Expr::Lit(l) => match &l.lit {
                    Lit::Str(s) => Some(s.value()),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        })
        .map(|l| l.strip_prefix(' ').unwrap_or(&l).trim_end().to_string())
        .collect();
    lines.join("\n").trim().to_string()
}

fn type_name(ty: &Type) -> String {
    match ty {
        Type::Path(p) => p
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect::<Vec<_>>()
            .join("::"),
        Type::Array(a) => match eval(&a.len) {
            Some(n) => format!("[{}; {n}]", type_name(&a.elem)),
            None => format!("[{}; ?]", type_name(&a.elem)),
        },
        Type::Reference(r) => format!(
            "&{}{}",
            if r.mutability.is_some() { "mut " } else { "" },
            type_name(&r.elem)
        ),
        _ => "?".to_string(),
    }
}

/// (size, alignment) of integer types and arrays of them, as `repr(C)` lays them out.
fn layout(ty: &Type) -> Option<(u32, u32)> {
    match ty {
        Type::Path(p) if p.qself.is_none() => {
            let n = match p.path.get_ident()?.to_string().as_str() {
                "i8" | "u8" => 1,
                "i16" | "u16" => 2,
                "i32" | "u32" => 4,
                "i64" | "u64" => 8,
                _ => return None,
            };
            Some((n, n))
        }
        Type::Array(a) => {
            let (size, align) = layout(&a.elem)?;
            let len = u32::try_from(eval(&a.len)?).ok()?;
            Some((size * len, align))
        }
        _ => None,
    }
}

fn eval(e: &Expr) -> Option<i64> {
    match e {
        Expr::Lit(l) => match &l.lit {
            Lit::Int(i) => i.base10_parse().ok(),
            _ => None,
        },
        Expr::Paren(p) => eval(&p.expr),
        Expr::Unary(u) if matches!(u.op, UnOp::Neg(_)) => eval(&u.expr).map(|v| -v),
        Expr::Binary(b) => {
            let (l, r) = (eval(&b.left)?, eval(&b.right)?);
            match b.op {
                BinOp::Shl(_) => l.checked_shl(u32::try_from(r).ok()?),
                BinOp::BitOr(_) => Some(l | r),
                BinOp::Add(_) => l.checked_add(r),
                BinOp::Mul(_) => l.checked_mul(r),
                _ => None,
            }
        }
        _ => None,
    }
}

fn expr_text(e: &Expr) -> Option<String> {
    match e {
        Expr::Lit(l) => match &l.lit {
            Lit::Int(i) => Some(i.base10_digits().to_string()),
            Lit::Str(s) => Some(format!("{:?}", s.value())),
            _ => None,
        },
        Expr::Paren(p) => Some(format!("({})", expr_text(&p.expr)?)),
        Expr::Unary(u) if matches!(u.op, UnOp::Neg(_)) => Some(format!("-{}", expr_text(&u.expr)?)),
        Expr::Binary(b) => {
            let op = match b.op {
                BinOp::Shl(_) => "<<",
                BinOp::BitOr(_) => "|",
                BinOp::Add(_) => "+",
                BinOp::Mul(_) => "*",
                _ => return None,
            };
            Some(format!(
                "{} {op} {}",
                expr_text(&b.left)?,
                expr_text(&b.right)?
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_keep_their_docs_and_values() {
        let m = Module::parse(
            "//! Top.\n/// Two lines\n/// of doc.\npub const S_X: i32 = 1 << 3;\npub const N: &str = \"ibc\";",
            "test",
        )
        .unwrap();
        assert_eq!(m.doc, "Top.");
        assert_eq!(m.consts[0].doc, "Two lines\nof doc.");
        assert_eq!(
            (m.consts[0].expr.as_str(), m.consts[0].value),
            ("1 << 3", Some(8))
        );
        assert_eq!(
            (m.consts[1].ty.as_str(), m.consts[1].expr.as_str()),
            ("&str", "\"ibc\"")
        );
        assert_eq!(m.name_of("S_", 8), Some("S_X"));
    }

    #[test]
    fn repr_c_layout_pads_like_the_compiler() {
        let m = Module::parse(
            "#[repr(C)] pub struct R { a: u8, b: i32, c: [u16; 3] }\npub struct P { v: Vec<i32> }",
            "test",
        )
        .unwrap();
        let r = m.record("R").unwrap();
        let offsets: Vec<u32> = r.fields.iter().map(|f| f.offset).collect();
        assert_eq!((offsets, r.size), (vec![0, 4, 8], 16));
        assert!(!m.record("P").unwrap().repr_c);
    }
}
