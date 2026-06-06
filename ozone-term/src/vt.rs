//! Minimal VT/ANSI terminal emulator: a character grid + cursor that interprets
//! the escape sequences a shell emits, instead of stripping them.
//!
//! Scope: printable text, `\r` `\n` `\b` `\t`, and the common CSI sequences —
//! cursor movement (CUU/CUD/CUF/CUB/CHA/VPA/CUP), erase in display/line (ED/EL).
//! SGR colors are parsed and ignored for now (the buffer is plain text); a
//! coloured render is a later step. OSC and other escapes are consumed.
//!
//! The screen scrolls into a capped scrollback; [`Vt::render`] returns the
//! scrollback + screen as text (trailing spaces trimmed), ready for display.

const MAX_SCROLLBACK: usize = 5000;
const TAB: usize = 8;

#[derive(Clone, Copy, PartialEq)]
enum State {
    Ground,
    Esc,
    Csi,
    Osc,
    /// Consume exactly one following byte (e.g. charset-select `ESC ( X`).
    SkipOne,
}

pub struct Vt {
    cols: usize,
    rows: usize,
    screen: Vec<Vec<char>>, // rows x cols
    scrollback: Vec<String>,
    cx: usize,
    cy: usize,
    state: State,
    params: Vec<u32>,
    cur_param: Option<u32>,
}

impl Vt {
    pub fn new(cols: usize, rows: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Self {
            cols,
            rows,
            screen: vec![vec![' '; cols]; rows],
            scrollback: Vec::new(),
            cx: 0,
            cy: 0,
            state: State::Ground,
            params: Vec::new(),
            cur_param: None,
        }
    }

    /// Feed a chunk of shell output through the emulator.
    pub fn process(&mut self, text: &str) {
        for ch in text.chars() {
            match self.state {
                State::Ground => self.ground(ch),
                State::Esc => self.esc(ch),
                State::Csi => self.csi(ch),
                State::Osc => {
                    // Consume until BEL or ST (ESC \). Approximate: end on BEL or ESC.
                    if ch == '\u{7}' {
                        self.state = State::Ground;
                    } else if ch == '\u{1b}' {
                        self.state = State::Ground; // swallow the following '\' next round if any
                    }
                }
                State::SkipOne => self.state = State::Ground,
            }
        }
    }

    fn ground(&mut self, ch: char) {
        match ch {
            '\u{1b}' => self.state = State::Esc,
            '\r' => self.cx = 0,
            '\n' | '\u{b}' | '\u{c}' => self.linefeed(),
            '\u{8}' => {
                self.cx = self.cx.saturating_sub(1);
            }
            '\t' => {
                self.cx = (((self.cx / TAB) + 1) * TAB).min(self.cols - 1);
            }
            c if (c as u32) < 0x20 || c == '\u{7f}' => {} // other controls: ignore
            c => self.put(c),
        }
    }

    fn esc(&mut self, ch: char) {
        match ch {
            '[' => {
                self.state = State::Csi;
                self.params.clear();
                self.cur_param = None;
            }
            ']' => self.state = State::Osc,
            '(' | ')' | '*' | '+' => self.state = State::SkipOne,
            'M' => {
                // Reverse index: move up, scrolling down at the top.
                if self.cy == 0 {
                    self.scroll_down();
                } else {
                    self.cy -= 1;
                }
                self.state = State::Ground;
            }
            _ => self.state = State::Ground,
        }
    }

    fn csi(&mut self, ch: char) {
        match ch {
            '0'..='9' => {
                let d = ch as u32 - '0' as u32;
                self.cur_param = Some(self.cur_param.unwrap_or(0) * 10 + d);
            }
            ';' => {
                self.params.push(self.cur_param.take().unwrap_or(0));
            }
            '?' | '>' | '!' | '=' => {} // private markers: ignore
            '\u{40}'..='\u{7e}' => {
                if let Some(p) = self.cur_param.take() {
                    self.params.push(p);
                }
                self.dispatch_csi(ch);
                self.state = State::Ground;
            }
            _ => {}
        }
    }

    fn param(&self, i: usize, default: u32) -> u32 {
        match self.params.get(i).copied() {
            Some(0) | None => default,
            Some(v) => v,
        }
    }

    fn dispatch_csi(&mut self, final_ch: char) {
        match final_ch {
            'A' => self.cy = self.cy.saturating_sub(self.param(0, 1) as usize),
            'B' | 'e' => self.cy = (self.cy + self.param(0, 1) as usize).min(self.rows - 1),
            'C' | 'a' => self.cx = (self.cx + self.param(0, 1) as usize).min(self.cols - 1),
            'D' => self.cx = self.cx.saturating_sub(self.param(0, 1) as usize),
            'E' => {
                self.cy = (self.cy + self.param(0, 1) as usize).min(self.rows - 1);
                self.cx = 0;
            }
            'F' => {
                self.cy = self.cy.saturating_sub(self.param(0, 1) as usize);
                self.cx = 0;
            }
            'G' | '`' => self.cx = (self.param(0, 1) as usize - 1).min(self.cols - 1),
            'd' => self.cy = (self.param(0, 1) as usize - 1).min(self.rows - 1),
            'H' | 'f' => {
                self.cy = (self.param(0, 1) as usize - 1).min(self.rows - 1);
                self.cx = (self.param(1, 1) as usize - 1).min(self.cols - 1);
            }
            'J' => self.erase_display(self.param(0, 0)),
            'K' => self.erase_line(self.param(0, 0)),
            // SGR (colors) and everything else: ignored for now.
            _ => {}
        }
    }

    fn erase_display(&mut self, mode: u32) {
        match mode {
            0 => {
                // cursor → end of screen
                for c in self.cx..self.cols {
                    self.screen[self.cy][c] = ' ';
                }
                for r in (self.cy + 1)..self.rows {
                    for c in 0..self.cols {
                        self.screen[r][c] = ' ';
                    }
                }
            }
            1 => {
                for r in 0..self.cy {
                    for c in 0..self.cols {
                        self.screen[r][c] = ' ';
                    }
                }
                for c in 0..=self.cx.min(self.cols - 1) {
                    self.screen[self.cy][c] = ' ';
                }
            }
            _ => {
                // 2 (and 3): clear whole screen.
                for row in &mut self.screen {
                    for c in row.iter_mut() {
                        *c = ' ';
                    }
                }
                if mode == 3 {
                    self.scrollback.clear();
                }
            }
        }
    }

    fn erase_line(&mut self, mode: u32) {
        let row = &mut self.screen[self.cy];
        match mode {
            0 => {
                for c in self.cx..self.cols {
                    row[c] = ' ';
                }
            }
            1 => {
                for c in 0..=self.cx.min(self.cols - 1) {
                    row[c] = ' ';
                }
            }
            _ => {
                for c in row.iter_mut() {
                    *c = ' ';
                }
            }
        }
    }

    fn put(&mut self, ch: char) {
        if self.cx >= self.cols {
            self.cx = 0;
            self.linefeed();
        }
        self.screen[self.cy][self.cx] = ch;
        self.cx += 1;
    }

    fn linefeed(&mut self) {
        if self.cy + 1 >= self.rows {
            self.scroll_up();
        } else {
            self.cy += 1;
        }
    }

    fn scroll_up(&mut self) {
        let top = self.screen.remove(0);
        self.scrollback.push(trim_row(&top));
        if self.scrollback.len() > MAX_SCROLLBACK {
            let excess = self.scrollback.len() - MAX_SCROLLBACK;
            self.scrollback.drain(0..excess);
        }
        self.screen.push(vec![' '; self.cols]);
    }

    fn scroll_down(&mut self) {
        self.screen.pop();
        self.screen.insert(0, vec![' '; self.cols]);
    }

    /// Resize the grid, preserving overlapping content; clamps the cursor.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }
        let mut next = vec![vec![' '; cols]; rows];
        for r in 0..rows.min(self.rows) {
            for c in 0..cols.min(self.cols) {
                next[r][c] = self.screen[r][c];
            }
        }
        self.screen = next;
        self.cols = cols;
        self.rows = rows;
        self.cx = self.cx.min(cols - 1);
        self.cy = self.cy.min(rows - 1);
    }

    /// The full visible text: scrollback then the screen, trailing spaces trimmed.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for line in &self.scrollback {
            out.push_str(line);
            out.push('\n');
        }
        for (i, row) in self.screen.iter().enumerate() {
            out.push_str(&trim_row(row));
            if i + 1 < self.screen.len() {
                out.push('\n');
            }
        }
        out
    }
}

fn trim_row(row: &[char]) -> String {
    let mut end = row.len();
    while end > 0 && row[end - 1] == ' ' {
        end -= 1;
    }
    row[..end].iter().collect()
}

#[cfg(test)]
mod tests {
    use super::Vt;

    fn render(input: &str) -> String {
        let mut vt = Vt::new(20, 5);
        vt.process(input);
        vt.render()
    }

    #[test]
    fn plain_text_and_newlines() {
        assert_eq!(render("ab\r\ncd"), "ab\ncd\n\n\n");
    }

    #[test]
    fn carriage_return_overwrites() {
        // "abc\rX" -> cursor home then write X over 'a'
        assert_eq!(render("abc\rX").lines().next().unwrap(), "Xbc");
    }

    #[test]
    fn backspace_moves_left() {
        // type "ab", backspace, "c" -> "ac"
        assert_eq!(render("ab\u{8}c").lines().next().unwrap(), "ac");
    }

    #[test]
    fn csi_cursor_position_and_write() {
        // ESC[2;3H moves to row2,col3 then writes X
        let out = render("\u{1b}[2;3HX");
        assert_eq!(out.lines().nth(1).unwrap(), "  X");
    }

    #[test]
    fn erase_display_clears_screen() {
        let out = render("hello\u{1b}[2Jworld");
        // after clear, "world" written from where cursor was (col 5 row 0)
        assert!(out.contains("world"));
        assert!(!out.contains("hello"));
    }

    #[test]
    fn sgr_colors_are_ignored_not_shown() {
        assert_eq!(render("\u{1b}[31mred\u{1b}[0m").lines().next().unwrap(), "red");
    }

    #[test]
    fn scrolls_into_scrollback() {
        let mut vt = Vt::new(4, 2);
        vt.process("a\r\nb\r\nc"); // 3 lines into a 2-row screen
        let out = vt.render();
        assert!(out.starts_with("a\n"));
        assert!(out.contains('c'));
    }
}
