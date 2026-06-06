//! Minimal ANSI escape stripping (no VT emulation yet).
//!
//! Removes CSI (`ESC [ … final`), OSC (`ESC ] … BEL/ST`), and other two-char
//! escapes; drops carriage returns and stray control chars; keeps printable
//! text plus `\n` and `\t`. Good enough to show clean command output until a
//! real VT parser (colors/cursor) lands behind the same terminal API.

/// Strip ANSI escapes and control noise from `input`.
pub fn strip(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    // CSI: consume params until a final byte 0x40..=0x7e.
                    while let Some(&n) = chars.peek() {
                        chars.next();
                        if ('\u{40}'..='\u{7e}').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    // OSC: consume until BEL or the ESC \ string terminator.
                    while let Some(&n) = chars.peek() {
                        if n == '\u{07}' {
                            chars.next();
                            break;
                        }
                        if n == '\u{1b}' {
                            chars.next();
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                        chars.next();
                    }
                }
                Some(_) => {
                    chars.next(); // two-char escape, drop the second char too
                }
                None => {}
            },
            '\r' => {} // drop carriage returns (Windows CRLF → LF)
            '\n' | '\t' => out.push(c),
            c if !c.is_control() => out.push(c),
            _ => {} // drop other control chars (bell, etc.)
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::strip;

    #[test]
    fn strips_csi_color_codes() {
        // "\x1b[31mred\x1b[0m" -> "red"
        assert_eq!(strip("\u{1b}[31mred\u{1b}[0m"), "red");
    }

    #[test]
    fn strips_cursor_moves_and_clears() {
        assert_eq!(strip("a\u{1b}[2Kb\u{1b}[Hc"), "abc");
    }

    #[test]
    fn strips_osc_title() {
        assert_eq!(strip("\u{1b}]0;my title\u{07}done"), "done");
    }

    #[test]
    fn keeps_newlines_tabs_drops_cr() {
        assert_eq!(strip("a\r\nb\tc"), "a\nb\tc");
    }

    #[test]
    fn keeps_unicode() {
        assert_eq!(strip("héllo\u{1b}[0m"), "héllo");
    }
}
