//! Document symbol extraction for the symbol picker / outline.
//!
//! Rust and TOML: backed by sylven's token-level `derive_symbols`. Other
//! filetypes still use line-based heuristics and can be upgraded later.

use sylven::SymbolKind as SylvenKind;

use crate::{Filetype, byte_to_line, parse_features};

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
        Filetype::Rust | Filetype::Toml => sylven_symbols(filetype, text),
        Filetype::Markdown => markdown_symbols(text),
        Filetype::Json | Filetype::Plain => Vec::new(),
    }
}

fn sylven_symbols(filetype: Filetype, text: &str) -> Vec<Symbol> {
    let features = match parse_features(filetype, text) {
        Some(f) => f,
        None => return Vec::new(),
    };
    features
        .symbols
        .into_iter()
        .map(|sym| Symbol {
            name: sym.name,
            kind: sylven_kind_to_local(sym.kind),
            line: byte_to_line(text, sym.name_range.start().to_usize()),
        })
        .collect()
}

fn sylven_kind_to_local(k: SylvenKind) -> SymbolKind {
    match k {
        SylvenKind::Function => SymbolKind::Function,
        SylvenKind::Struct => SymbolKind::Struct,
        SylvenKind::Enum => SymbolKind::Enum,
        SylvenKind::Trait => SymbolKind::Trait,
        SylvenKind::Impl => SymbolKind::Impl,
        SylvenKind::Module => SymbolKind::Module,
        SylvenKind::Constant => SymbolKind::Constant,
        SylvenKind::TypeAlias => SymbolKind::TypeAlias,
        SylvenKind::Macro => SymbolKind::Macro,
        SylvenKind::Section => SymbolKind::Section,
    }
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
        // sylven takes the token right after `impl` as the name; for
        // `impl Trait for Bravo` that's `Trait`, not `Bravo`.
        assert!(got.contains(&("Trait", SymbolKind::Impl, 4)));
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
