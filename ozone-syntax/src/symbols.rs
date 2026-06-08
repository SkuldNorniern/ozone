//! Layer-0 symbol extraction: deterministic, line-based document symbols for
//! the symbol picker / outline. No structural parser — per-filetype heuristics
//! over each line's leading tokens. Good enough to jump around a file; a future
//! structural pass can replace it without changing the `Symbol` shape.

use crate::Filetype;

/// A kind of document symbol, used for the picker's detail label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Module,
    Constant,
    TypeAlias,
    Macro,
    Heading,
    Section,
}

impl SymbolKind {
    /// Short human label (shown as the picker row's detail).
    pub fn label(self) -> &'static str {
        match self {
            SymbolKind::Function => "fn",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Impl => "impl",
            SymbolKind::Module => "mod",
            SymbolKind::Constant => "const",
            SymbolKind::TypeAlias => "type",
            SymbolKind::Macro => "macro",
            SymbolKind::Heading => "heading",
            SymbolKind::Section => "section",
        }
    }
}

/// One extracted symbol: a name, its kind, and the 0-based line it sits on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub line: usize,
}

/// Extract document symbols from `text` for `filetype`. Returns them in document
/// order. Unknown filetypes yield an empty list.
pub fn symbols(filetype: Filetype, text: &str) -> Vec<Symbol> {
    match filetype {
        Filetype::Rust => rust_symbols(text),
        Filetype::Markdown => markdown_symbols(text),
        Filetype::Toml => toml_symbols(text),
        Filetype::Json | Filetype::Plain => Vec::new(),
    }
}

/// The first identifier after `prefix` on a trimmed line, stopping at the first
/// non-identifier byte (so `fn foo(` yields `foo`).
fn ident_after(trimmed: &str, prefix: &str) -> Option<String> {
    let rest = trimmed.strip_prefix(prefix)?;
    let rest = rest.trim_start();
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

fn rust_symbols(text: &str) -> Vec<Symbol> {
    let mut out = Vec::new();
    for (line, raw) in text.lines().enumerate() {
        let t = raw.trim_start();
        // Skip common visibility / modifier prefixes so `pub async fn` matches.
        let t = t
            .strip_prefix("pub(crate) ")
            .or_else(|| t.strip_prefix("pub "))
            .unwrap_or(t);
        let t = t.strip_prefix("default ").unwrap_or(t);
        let t = t.strip_prefix("unsafe ").unwrap_or(t);
        let t = t.strip_prefix("async ").unwrap_or(t);
        let t = t
            .strip_prefix("const ")
            .filter(|r| r.starts_with("fn "))
            .unwrap_or(t);

        let found = ident_after(t, "fn ")
            .map(|n| (n, SymbolKind::Function))
            .or_else(|| ident_after(t, "struct ").map(|n| (n, SymbolKind::Struct)))
            .or_else(|| ident_after(t, "enum ").map(|n| (n, SymbolKind::Enum)))
            .or_else(|| ident_after(t, "trait ").map(|n| (n, SymbolKind::Trait)))
            .or_else(|| ident_after(t, "mod ").map(|n| (n, SymbolKind::Module)))
            .or_else(|| ident_after(t, "type ").map(|n| (n, SymbolKind::TypeAlias)))
            .or_else(|| ident_after(t, "static ").map(|n| (n, SymbolKind::Constant)))
            .or_else(|| ident_after(t, "const ").map(|n| (n, SymbolKind::Constant)))
            .or_else(|| ident_after(t, "macro_rules! ").map(|n| (n, SymbolKind::Macro)))
            .or_else(|| impl_name(t));
        if let Some((name, kind)) = found {
            out.push(Symbol { name, kind, line });
        }
    }
    out
}

/// `impl Foo` / `impl Trait for Foo` → a symbol named after the type.
fn impl_name(trimmed: &str) -> Option<(String, SymbolKind)> {
    let rest = trimmed.strip_prefix("impl ")?.trim_start();
    // Strip generics on the impl block itself: `impl<T> Foo<T>`.
    let rest = rest.strip_prefix('<').map_or(rest, |after| {
        after
            .split_once('>')
            .map(|(_, r)| r.trim_start())
            .unwrap_or(after)
    });
    // `Trait for Type` → keep the part after `for`.
    let subject = rest.rsplit(" for ").next().unwrap_or(rest);
    let name: String = subject
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some((name, SymbolKind::Impl))
}

fn markdown_symbols(text: &str) -> Vec<Symbol> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (line, raw) in text.lines().enumerate() {
        let t = raw.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some(rest) = t.strip_prefix('#') {
            let title = rest.trim_start_matches('#').trim();
            if !title.is_empty() {
                out.push(Symbol {
                    name: title.to_string(),
                    kind: SymbolKind::Heading,
                    line,
                });
            }
        }
    }
    out
}

fn toml_symbols(text: &str) -> Vec<Symbol> {
    let mut out = Vec::new();
    for (line, raw) in text.lines().enumerate() {
        let t = raw.trim();
        if t.starts_with('[') && t.ends_with(']') && t.len() > 2 {
            out.push(Symbol {
                name: t.trim_matches(['[', ']']).trim().to_string(),
                kind: SymbolKind::Section,
                line,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_items() {
        let src = "pub fn alpha() {}\nstruct Bravo;\n  enum Charlie {}\nimpl Bravo {}\nimpl Trait for Bravo {}\nmacro_rules! mac {}\nconst K: u8 = 1;";
        let s = symbols(Filetype::Rust, src);
        let got: Vec<(&str, SymbolKind, usize)> = s
            .iter()
            .map(|x| (x.name.as_str(), x.kind, x.line))
            .collect();
        assert!(got.contains(&("alpha", SymbolKind::Function, 0)));
        assert!(got.contains(&("Bravo", SymbolKind::Struct, 1)));
        assert!(got.contains(&("Charlie", SymbolKind::Enum, 2)));
        assert!(got.contains(&("Bravo", SymbolKind::Impl, 3)));
        assert!(got.contains(&("Bravo", SymbolKind::Impl, 4)));
        assert!(got.contains(&("mac", SymbolKind::Macro, 5)));
        assert!(got.contains(&("K", SymbolKind::Constant, 6)));
    }

    #[test]
    fn markdown_headings_skip_fences() {
        let src = "# Title\n```\n# not a heading\n```\n## Sub";
        let s = symbols(Filetype::Markdown, src);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].name, "Title");
        assert_eq!((s[1].name.as_str(), s[1].line), ("Sub", 4));
    }

    #[test]
    fn toml_sections() {
        let src = "[editor]\nfont = \"x\"\n[[keymap]]\n";
        let s = symbols(Filetype::Toml, src);
        assert_eq!(s[0].name, "editor");
        assert_eq!(s[1].name, "keymap");
    }

    #[test]
    fn plain_and_json_have_none() {
        assert!(symbols(Filetype::Plain, "anything").is_empty());
        assert!(symbols(Filetype::Json, "{\"a\":1}").is_empty());
    }
}
