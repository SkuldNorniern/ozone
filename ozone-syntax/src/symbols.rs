//! Document symbol extraction for the symbol picker / outline.
//!
//! Rust, TOML, and Markdown: backed by sylven's token-level `derive_symbols`.
//! JSON and Plain have no symbols.

use sylven::SymbolKind as SylvenKind;
use taste::Language;

use crate::{LineIndex, parse_features};

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

/// Extract document symbols from `text` for `lang`. Returns them in document
/// order. Unknown languages yield an empty list.
pub fn symbols(lang: Option<Language>, text: &str) -> Vec<Symbol> {
    match lang {
        Some(Language::RUST | Language::TOML | Language::MARKDOWN | Language::YAML) => {
            sylven_symbols(lang, text)
        }
        _ => Vec::new(),
    }
}

fn sylven_symbols(lang: Option<Language>, text: &str) -> Vec<Symbol> {
    let features = match parse_features(lang, text) {
        Some(f) => f,
        None => return Vec::new(),
    };
    let lines = LineIndex::new(text);
    features
        .symbols
        .iter()
        .map(|sym| Symbol {
            name: sym.name.clone(),
            kind: sylven_kind_to_local(sym.kind),
            line: lines.line_of(sym.name_range.start().to_usize()),
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
        SylvenKind::Heading => SymbolKind::Heading,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_items() {
        let src = "pub fn alpha() {}\nstruct Bravo;\n  enum Charlie {}\nimpl Bravo {}\nimpl Trait for Bravo {}\nmacro_rules! mac {}\nconst K: u8 = 1;";
        let s = symbols(Some(Language::RUST), src);
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
        let s = symbols(Some(Language::MARKDOWN), src);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].name, "Title");
        assert_eq!((s[1].name.as_str(), s[1].line), ("Sub", 4));
    }

    #[test]
    fn toml_sections() {
        let src = "[editor]\nfont = \"x\"\n[[keymap]]\n";
        let s = symbols(Some(Language::TOML), src);
        assert_eq!(s[0].name, "editor");
        assert_eq!(s[1].name, "keymap");
    }

    #[test]
    fn plain_and_json_have_none() {
        assert!(symbols(None, "anything").is_empty());
        assert!(symbols(Some(Language::JSON), "{\"a\":1}").is_empty());
    }
}
