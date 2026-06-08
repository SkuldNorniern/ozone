//! A small, dependency-free JSON value + parser + serializer.
//!
//! Just enough JSON for the LSP wire protocol (RFC 8259), in the same
//! hand-rolled spirit as Ozone's TOML config parsing — no serde, no external
//! crates. Objects keep insertion order in a `Vec` (LSP messages are small;
//! ordered lookup is fine and avoids a `HashMap` import). Numbers are `f64`.

use std::fmt::{self, Write as _};

/// A parsed JSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Array(Vec<Json>),
    /// Ordered key/value pairs.
    Object(Vec<(String, Json)>),
}

impl Json {
    /// Value for `key` in an object, or `None`.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// Numeric value as an `i64` (truncating). `None` for non-numbers.
    pub fn as_i64(&self) -> Option<i64> {
        self.as_f64().map(|n| n as i64)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }
}

impl fmt::Display for Json {
    /// Compact JSON. `to_string()` comes from this `Display` impl.
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Json::Null => out.write_str("null"),
            Json::Bool(true) => out.write_str("true"),
            Json::Bool(false) => out.write_str("false"),
            Json::Num(n) => {
                if n.fract() == 0.0 && n.is_finite() {
                    write!(out, "{}", *n as i64)
                } else {
                    write!(out, "{n}")
                }
            }
            Json::Str(s) => write_json_string(s, out),
            Json::Array(items) => {
                out.write_char('[')?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.write_char(',')?;
                    }
                    write!(out, "{item}")?;
                }
                out.write_char(']')
            }
            Json::Object(pairs) => {
                out.write_char('{')?;
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.write_char(',')?;
                    }
                    write_json_string(k, out)?;
                    out.write_char(':')?;
                    write!(out, "{v}")?;
                }
                out.write_char('}')
            }
        }
    }
}

fn write_json_string(s: &str, out: &mut impl fmt::Write) -> fmt::Result {
    out.write_char('"')?;
    for c in s.chars() {
        match c {
            '"' => out.write_str("\\\"")?,
            '\\' => out.write_str("\\\\")?,
            '\n' => out.write_str("\\n")?,
            '\r' => out.write_str("\\r")?,
            '\t' => out.write_str("\\t")?,
            '\u{08}' => out.write_str("\\b")?,
            '\u{0c}' => out.write_str("\\f")?,
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32)?,
            c => out.write_char(c)?,
        }
    }
    out.write_char('"')
}

/// Parse a JSON document. Trailing whitespace is allowed; trailing non-space
/// content is an error.
pub fn parse(input: &str) -> Result<Json, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut p = Parser {
        chars: &chars,
        pos: 0,
    };
    p.skip_ws();
    let value = p.value()?;
    p.skip_ws();
    if p.pos != p.chars.len() {
        return Err(format!("trailing content at position {}", p.pos));
    }
    Ok(value)
}

struct Parser<'a> {
    chars: &'a [char],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, c: char) -> Result<(), String> {
        if self.peek() == Some(c) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("expected '{c}' at position {}", self.pos))
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.skip_ws();
        match self.peek() {
            Some('{') => self.object(),
            Some('[') => self.array(),
            Some('"') => Ok(Json::Str(self.string()?)),
            Some('t') | Some('f') => self.boolean(),
            Some('n') => self.null(),
            Some(c) if c == '-' || c.is_ascii_digit() => self.number(),
            Some(c) => Err(format!("unexpected '{c}' at position {}", self.pos)),
            None => Err("unexpected end of input".to_string()),
        }
    }

    fn keyword(&mut self, word: &str) -> Result<(), String> {
        for expected in word.chars() {
            if self.bump() != Some(expected) {
                return Err(format!("invalid literal near position {}", self.pos));
            }
        }
        Ok(())
    }

    fn null(&mut self) -> Result<Json, String> {
        self.keyword("null")?;
        Ok(Json::Null)
    }

    fn boolean(&mut self) -> Result<Json, String> {
        if self.peek() == Some('t') {
            self.keyword("true")?;
            Ok(Json::Bool(true))
        } else {
            self.keyword("false")?;
            Ok(Json::Bool(false))
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.pos;
        if self.peek() == Some('-') {
            self.pos += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
        {
            self.pos += 1;
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        text.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("invalid number '{text}'"))
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect('"')?;
        let mut s = String::new();
        loop {
            match self.bump() {
                None => return Err("unterminated string".to_string()),
                Some('"') => return Ok(s),
                Some('\\') => match self.bump() {
                    Some('"') => s.push('"'),
                    Some('\\') => s.push('\\'),
                    Some('/') => s.push('/'),
                    Some('n') => s.push('\n'),
                    Some('r') => s.push('\r'),
                    Some('t') => s.push('\t'),
                    Some('b') => s.push('\u{08}'),
                    Some('f') => s.push('\u{0c}'),
                    Some('u') => s.push(self.unicode_escape()?),
                    other => return Err(format!("invalid escape '\\{other:?}'")),
                },
                Some(c) => s.push(c),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let mut value = 0u32;
        for _ in 0..4 {
            let c = self.bump().ok_or("truncated \\u escape")?;
            let digit = c.to_digit(16).ok_or(format!("bad hex digit '{c}'"))?;
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn unicode_escape(&mut self) -> Result<char, String> {
        let hi = self.hex4()?;
        // Combine a UTF-16 surrogate pair if present.
        if (0xD800..=0xDBFF).contains(&hi) {
            if self.bump() != Some('\\') || self.bump() != Some('u') {
                return Err("expected low surrogate".to_string());
            }
            let lo = self.hex4()?;
            let combined = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
            char::from_u32(combined).ok_or_else(|| "invalid surrogate pair".to_string())
        } else {
            char::from_u32(hi).ok_or_else(|| "invalid \\u code point".to_string())
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.expect('[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(']') {
            self.pos += 1;
            return Ok(Json::Array(items));
        }
        loop {
            items.push(self.value()?);
            self.skip_ws();
            match self.bump() {
                Some(',') => self.skip_ws(),
                Some(']') => return Ok(Json::Array(items)),
                _ => return Err(format!("expected ',' or ']' at position {}", self.pos)),
            }
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.expect('{')?;
        let mut pairs = Vec::new();
        self.skip_ws();
        if self.peek() == Some('}') {
            self.pos += 1;
            return Ok(Json::Object(pairs));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            self.skip_ws();
            self.expect(':')?;
            let value = self.value()?;
            pairs.push((key, value));
            self.skip_ws();
            match self.bump() {
                Some(',') => {}
                Some('}') => return Ok(Json::Object(pairs)),
                _ => return Err(format!("expected ',' or '}}' at position {}", self.pos)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_primitives() {
        assert_eq!(parse("null").unwrap(), Json::Null);
        assert_eq!(parse("true").unwrap(), Json::Bool(true));
        assert_eq!(parse("false").unwrap(), Json::Bool(false));
        assert_eq!(parse("  42 ").unwrap(), Json::Num(42.0));
        assert_eq!(parse("-3.5e2").unwrap(), Json::Num(-350.0));
        assert_eq!(parse("\"hi\"").unwrap(), Json::Str("hi".into()));
    }

    #[test]
    fn parses_nested() {
        let j = parse(r#"{"a":[1,2,{"b":true}],"c":null}"#).unwrap();
        assert_eq!(j.get("a").unwrap().as_array().unwrap().len(), 3);
        assert_eq!(
            j.get("a").unwrap().as_array().unwrap()[2].get("b"),
            Some(&Json::Bool(true))
        );
        assert!(j.get("c").unwrap().is_null());
    }

    #[test]
    fn string_escapes_roundtrip() {
        let j = parse(r#""line\nbreak \"q\" \t tab \\ slash""#).unwrap();
        assert_eq!(j.as_str().unwrap(), "line\nbreak \"q\" \t tab \\ slash");
        // re-serialize and re-parse → identical value
        assert_eq!(parse(&j.to_string()).unwrap(), j);
    }

    #[test]
    fn unicode_and_surrogate_pairs() {
        assert_eq!(parse(r#""é""#).unwrap().as_str(), Some("é"));
        // 😀 as a UTF-16 surrogate pair
        assert_eq!(parse(r#""😀""#).unwrap().as_str(), Some("😀"));
    }

    #[test]
    fn serialize_is_compact_and_ordered() {
        let j = Json::Object(vec![
            ("jsonrpc".into(), Json::Str("2.0".into())),
            ("id".into(), Json::Num(1.0)),
            (
                "nested".into(),
                Json::Array(vec![Json::Bool(false), Json::Null]),
            ),
        ]);
        assert_eq!(
            j.to_string(),
            r#"{"jsonrpc":"2.0","id":1,"nested":[false,null]}"#
        );
    }

    #[test]
    fn rejects_trailing_and_truncated() {
        assert!(parse("{} junk").is_err());
        assert!(parse("[1,2").is_err());
        assert!(parse("\"abc").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn empty_containers() {
        assert_eq!(parse("[]").unwrap(), Json::Array(vec![]));
        assert_eq!(parse("{}").unwrap(), Json::Object(vec![]));
        assert_eq!(parse(" { } ").unwrap(), Json::Object(vec![]));
    }
}
