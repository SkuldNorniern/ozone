//! Notification toasts + their controller (the popup "handle").
//!
//! [`Notifications`] is the single owner of the transient-message list: commands
//! and plugins post messages (via [`ozone_editor::UiIntent::Notify`], drained in
//! `apply_ui_intents`), the run loop [`tick`](Notifications::tick)s it each frame
//! to expire old ones, and the renderer draws the live stack. It is intentionally
//! a self-contained handle so it can later grow into the broader popup host
//! (hover cards, completion, progress) without the run loop changing shape.
//!
//! Rendering reuses the shared [`crate::components`] primitives; layout is a stack of
//! rounded cards anchored to the top-right, each with a severity accent stripe.

use std::time::{Duration, Instant};

use aurea::AureaResult;
use aurea::render::{DrawingContext, Font, Point, Rect};
use ozone_editor::NotifyLevel;

use crate::components::{draw_panel, top_right_rect};
use crate::theme::{notify_accent, palette, solid};

/// Max body lines a single toast shows before truncating with a "+N more"
/// marker, so a chatty command's full output (e.g. a failed `cargo build`
/// dumping a wall of warnings) can't grow one card past the screen and starve
/// the stack of room.
const MAX_BODY_LINES: usize = 8;

/// Default time a toast stays before auto-dismissing, by severity. Errors linger.
fn default_ttl(level: NotifyLevel) -> Duration {
    match level {
        NotifyLevel::Error => Duration::from_secs(8),
        NotifyLevel::Warn => Duration::from_secs(6),
        _ => Duration::from_secs(4),
    }
}

/// One live notification.
struct Notification {
    #[allow(dead_code)] // handle for a future dismiss-by-id / update API
    id: u64,
    level: NotifyLevel,
    text: String,
    born: Instant,
    ttl: Duration,
}

impl Notification {
    fn expired(&self, now: Instant) -> bool {
        now.duration_since(self.born) >= self.ttl
    }
}

/// Owns and renders the notification stack. The frontend holds exactly one.
#[derive(Default)]
pub(crate) struct Notifications {
    items: Vec<Notification>,
    next_id: u64,
    /// Cap so a chatty plugin can't grow the stack without bound.
    max: usize,
}

impl Notifications {
    pub(crate) fn new() -> Self {
        Self {
            items: Vec::new(),
            next_id: 1,
            max: 6,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Post a notification. `timeout_ms` `None` uses the per-severity default.
    /// Returns the assigned id (for a future dismiss/update API).
    pub(crate) fn push(
        &mut self,
        level: NotifyLevel,
        text: String,
        timeout_ms: Option<u64>,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let ttl = timeout_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| default_ttl(level));
        self.items.push(Notification {
            id,
            level,
            text,
            born: Instant::now(),
            ttl,
        });
        // Drop the oldest if we exceeded the cap.
        if self.items.len() > self.max {
            self.items.remove(0);
        }
        id
    }

    /// Drop expired notifications. Returns whether the list changed (so the run
    /// loop knows to repaint).
    pub(crate) fn tick(&mut self) -> bool {
        let now = Instant::now();
        let before = self.items.len();
        self.items.retain(|n| !n.expired(now));
        self.items.len() != before
    }

    /// Draw the stack of toasts, newest at the top-right, older below it.
    pub(crate) fn draw(
        &self,
        ctx: &mut dyn DrawingContext,
        font: &Font,
        width: f32,
        height: f32,
    ) -> AureaResult<()> {
        if self.items.is_empty() {
            return Ok(());
        }

        let metrics = ctx.measure_text("M", font).ok();
        let char_w = metrics
            .as_ref()
            .map(|m| m.advance)
            .unwrap_or(font.size * 0.6);
        let ascent = metrics
            .as_ref()
            .map(|m| m.ascent)
            .unwrap_or(font.size * 0.8);
        let descent = metrics
            .as_ref()
            .map(|m| m.descent)
            .unwrap_or(font.size * 0.2);
        let line_h = (font.size * 1.5).max(15.0);

        let margin = 12.0;
        let pad = 10.0;
        let stripe_w = 3.0;
        let card_w = (width * 0.32).clamp(180.0, 380.0);
        // Characters that fit on one text line inside the card.
        let inner_w = card_w - pad * 2.0 - stripe_w - 6.0;
        let cols = ((inner_w / char_w).floor() as usize).max(8);

        // Newest first (top), so iterate the list in reverse.
        let mut y = margin;
        for n in self.items.iter().rev() {
            let mut lines = wrap(&n.text, cols);
            if lines.len() > MAX_BODY_LINES {
                let remaining = lines.len() - (MAX_BODY_LINES - 1);
                lines.truncate(MAX_BODY_LINES - 1);
                lines.push(format!(
                    "… +{remaining} more line{}",
                    if remaining == 1 { "" } else { "s" }
                ));
            }
            let body_h = lines.len() as f32 * line_h;
            let card_h = body_h + pad * 2.0;
            let card = top_right_rect(width, card_w, card_h, margin);
            let card = Rect::new(card.x, y, card.width, card_h);
            if y + card_h > height {
                break; // ran out of vertical room
            }

            draw_panel(ctx, card, 6.0)?;
            // Severity accent stripe down the left edge.
            let accent = notify_accent(n.level);
            ctx.draw_rect(
                Rect::new(card.x, card.y + 4.0, stripe_w, card_h - 8.0),
                &solid(accent),
            )?;

            let text_x = card.x + stripe_w + 8.0;
            for (i, line) in lines.iter().enumerate() {
                let top = card.y + pad + i as f32 * line_h;
                let base = top + (line_h + ascent - descent) / 2.0;
                // First line in full strength, continuation lines dimmer.
                let color = if i == 0 {
                    palette().picker_fg
                } else {
                    palette().picker_detail
                };
                ctx.draw_text_with_font(line, Point::new(text_x, base), font, &solid(color))?;
            }

            y += card_h + 8.0;
        }
        Ok(())
    }
}

/// Greedy word-wrap to `cols` characters; falls back to hard splits for words
/// longer than a line. Returns at least one line.
fn wrap(text: &str, cols: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for raw in text.split('\n') {
        let mut line = String::new();
        for word in raw.split(' ') {
            let mut word = word;
            // Hard-split a word that can't fit on a line at all.
            while word.chars().count() > cols {
                let take: String = word.chars().take(cols).collect();
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                lines.push(take);
                word = &word[word
                    .char_indices()
                    .nth(cols)
                    .map(|(i, _)| i)
                    .unwrap_or(word.len())..];
            }
            let extra = if line.is_empty() { 0 } else { 1 };
            if line.chars().count() + extra + word.chars().count() > cols && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_cap() {
        let mut n = Notifications::new();
        for i in 0..10 {
            n.push(NotifyLevel::Info, format!("msg {i}"), None);
        }
        assert!(n.items.len() <= n.max);
        assert!(!n.is_empty());
    }

    #[test]
    fn zero_timeout_expires_immediately() {
        let mut n = Notifications::new();
        n.push(NotifyLevel::Info, "x".into(), Some(0));
        assert!(n.tick()); // expired this tick
        assert!(n.is_empty());
    }

    #[test]
    fn wrap_breaks_on_width() {
        let lines = wrap("hello there friend", 6);
        assert!(lines.iter().all(|l| l.chars().count() <= 6));
        assert_eq!(lines.join(" "), "hello there friend");
    }

    #[test]
    fn wrap_hard_splits_long_word() {
        let lines = wrap("abcdefghij", 4);
        assert_eq!(lines, vec!["abcd", "efgh", "ij"]);
    }
}
