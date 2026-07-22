//! A real, minimal web browser: types a URL, resolves it via real DNS,
//! opens a real TCP (or, for `https://`, a real TLS 1.3) connection, sends
//! a real HTTP/1.1 GET (`crate::net::http`), and renders whatever text the
//! server actually sent back -- a genuine fetch over the wire, not a
//! canned demo page.
//!
//! Honest scope: rendering is "strip HTML tags to plain text," not a real
//! layout/CSS engine -- moon OS has no DOM, no CSS box model, no image
//! decoder wired into this widget, no JavaScript. That's a deliberate,
//! documented line: a genuine HTML renderer is a project on the scale of
//! the whole GUI again. `https://` support has its own, more serious
//! honest gap: see `net::tls`'s module doc comment -- real encryption,
//! but no certificate chain-of-trust validation, so this is not secure
//! against an active attacker. What's real: the network round-trip, and
//! the fact that the text on screen is the actual bytes a real server
//! sent for the actual URL typed in.
//!
//! `fetch()` blocks the whole UI while the request is in flight -- this
//! kernel's network stack is synchronous/poll-driven end to end (see
//! `net`'s module doc comment), so there's no background-thread fetch to
//! hand this off to yet.

use crate::framebuffer;
use crate::net::http;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::Cell;

const ADDRESS_BAR_H: i32 = 18;
const STATUS_H: i32 = 14;
const LINE_HEIGHT: i32 = 12;

pub struct BrowserState {
    url: String,
    editing_url: bool,
    status: String,
    /// Word-wrapped lines of the last fetched page's tag-stripped text.
    lines: Vec<String>,
    scroll: usize,
    /// Content-area width in pixels from the most recent `render()` --
    /// needed to compute a word-wrap column count when a fetch is
    /// triggered from `handle_char`, which doesn't otherwise know the
    /// window's size. Interior mutability for the same reason as
    /// `StoreState::content_w`: `render` only gets `&self`.
    content_w: Cell<u32>,
}

impl BrowserState {
    pub fn new() -> Self {
        Self {
            url: String::from("example.com/"),
            editing_url: false,
            status: String::from("type an address and press Enter"),
            lines: Vec::new(),
            scroll: 0,
            content_w: Cell::new(300),
        }
    }

    fn fetch(&mut self) {
        let cols = ((self.content_w.get() as i32 - 8) / 8).max(1) as usize;
        self.status = format!("connecting to {}...", &self.url);
        match http::fetch(&self.url) {
            Ok(resp) => {
                let text = http::body_text(&resp);
                let stripped = strip_html(&text);
                self.lines = wrap_text(&stripped, cols);
                self.scroll = 0;
                self.status = format!("{} -- {} bytes", resp.status, resp.body.len());
            }
            Err(err) => {
                self.lines.clear();
                self.status = format!("failed: {err:?}");
            }
        }
    }

    pub fn handle_char(&mut self, ch: u8) {
        if !self.editing_url {
            return;
        }
        match ch {
            b'\n' => {
                self.editing_url = false;
                self.fetch();
            }
            0x08 => {
                self.url.pop();
            }
            0x20..=0x7E => self.url.push(ch as char),
            _ => {}
        }
    }

    /// `x`/`y` local to the content area, same convention as every other
    /// widget's `handle_click`. Clicking the address bar focuses it for
    /// typing; clicking the content area scrolls (top half up, bottom half
    /// down) -- a real, if simple, substitute for a scroll wheel/scrollbar.
    pub fn handle_click(&mut self, x: i32, y: i32) {
        let _ = x;
        if y < ADDRESS_BAR_H {
            self.editing_url = true;
            return;
        }
        self.editing_url = false;
        if y < ADDRESS_BAR_H + 40 {
            self.scroll = self.scroll.saturating_sub(3);
        } else {
            self.scroll = (self.scroll + 3).min(self.lines.len().saturating_sub(1));
        }
    }

    pub fn render(&self, x: i32, y: i32, w: u32, h: u32) {
        self.content_w.set(w);
        framebuffer::with(|c| {
            c.fill_rect(x, y, w, h, (0x12, 0x14, 0x1A));

            let bar_color = if self.editing_url {
                crate::gui::theme::accent()
            } else {
                (0x40, 0x44, 0x50)
            };
            c.fill_rect(
                x + 2,
                y + 2,
                w - 4,
                ADDRESS_BAR_H as u32 - 4,
                (0x08, 0x0A, 0x12),
            );
            c.glow_border(x + 2, y + 2, w - 4, ADDRESS_BAR_H as u32 - 4, bar_color);
            let shown = if self.editing_url {
                format!("{}_", self.url)
            } else {
                self.url.clone()
            };
            c.draw_str_at(x + 6, y + 5, &shown, (0xE0, 0xE0, 0xE0), None);

            let content_y = y + ADDRESS_BAR_H;
            let content_h = h as i32 - ADDRESS_BAR_H - STATUS_H;
            let visible_rows = (content_h / LINE_HEIGHT).max(1) as usize;
            for (row, line) in self
                .lines
                .iter()
                .skip(self.scroll)
                .take(visible_rows)
                .enumerate()
            {
                c.draw_str_at(
                    x + 4,
                    content_y + 4 + row as i32 * LINE_HEIGHT,
                    line,
                    (0xC8, 0xC8, 0xD0),
                    None,
                );
            }
            if self.lines.is_empty() {
                c.draw_str_at(
                    x + 4,
                    content_y + 4,
                    "(nothing loaded yet)",
                    (0x70, 0x74, 0x80),
                    None,
                );
            }

            let status_y = y + h as i32 - STATUS_H + 2;
            c.fill_rect(x, status_y - 2, w, STATUS_H as u32, (0x08, 0x08, 0x0C));
            c.draw_str_at(x + 4, status_y, &self.status, (0x80, 0x80, 0x90), None);
        });
    }
}

impl Default for BrowserState {
    fn default() -> Self {
        Self::new()
    }
}

/// Strips HTML tags down to plain text: drops everything between `<` and
/// `>`, skips `<script>`/`<style>` bodies entirely (their contents aren't
/// meant to be read as page text), and decodes the handful of entities
/// real pages actually use. Not a real HTML parser -- no DOM, no nesting
/// awareness beyond script/style, no malformed-markup recovery beyond
/// "if there's no closing `>`, stop." Genuinely turns real markup into
/// readable text for anything that isn't relying on more than that.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut chars = html.chars().peekable();
    let mut skip_until: Option<&str> = None;

    while let Some(ch) = chars.next() {
        if ch != '<' {
            out.push(ch);
            continue;
        }
        let mut tag = String::new();
        for c in chars.by_ref() {
            if c == '>' {
                break;
            }
            tag.push(c);
        }
        let lower = tag.to_ascii_lowercase();
        if let Some(end_tag) = skip_until {
            if lower.starts_with(&format!("/{end_tag}")) {
                skip_until = None;
            }
            continue;
        }
        if lower.starts_with("script") {
            skip_until = Some("script");
        } else if lower.starts_with("style") {
            skip_until = Some("style");
        } else if lower.starts_with("br") || lower.starts_with("/p") || lower.starts_with("/div") {
            out.push('\n');
        }
    }

    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

/// Word-wraps `text` to `cols` characters per line, collapsing runs of
/// blank lines down to one so a tag-heavy page doesn't render as mostly
/// empty space.
fn wrap_text(text: &str, cols: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut blank_run = false;
    for raw_line in text.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            if !blank_run {
                out.push(String::new());
            }
            blank_run = true;
            continue;
        }
        blank_run = false;
        let mut current = String::new();
        for word in trimmed.split_whitespace() {
            if !current.is_empty() && current.len() + 1 + word.len() > cols {
                out.push(current.clone());
                current.clear();
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
        if !current.is_empty() {
            out.push(current);
        }
    }
    out
}
