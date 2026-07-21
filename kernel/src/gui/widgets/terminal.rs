//! A tiny built-in terminal widget: a scrollback buffer, a single editable
//! input line, and a handful of commands. Not a real shell running a real
//! process (there's no usermode or ELF loader yet -- that's M7/M8) -- this
//! is a kernel-hosted widget proving the window system can host interactive
//! content and route keyboard input to it.

use crate::framebuffer;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

const MAX_LINES: usize = 200;
const LINE_HEIGHT: i32 = 10;

pub struct TerminalState {
    lines: VecDeque<String>,
    current: String,
}

impl TerminalState {
    pub fn new() -> Self {
        let mut lines = VecDeque::new();
        lines.push_back(String::from("moon OS terminal -- type 'help'"));
        Self {
            lines,
            current: String::new(),
        }
    }

    pub fn handle_char(&mut self, ch: u8) {
        match ch {
            b'\n' => {
                let line = core::mem::take(&mut self.current);
                self.push_line(alloc::format!("> {}", line));
                self.run_command(&line);
            }
            0x08 => {
                self.current.pop();
            }
            0x20..=0x7E => self.current.push(ch as char),
            _ => {}
        }
    }

    fn push_line(&mut self, line: String) {
        self.lines.push_back(line);
        while self.lines.len() > MAX_LINES {
            self.lines.pop_front();
        }
    }

    fn run_command(&mut self, line: &str) {
        let line = line.trim();
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("help") => {
                self.push_line(String::from(
                    "commands: help, clear, uptime, mem, echo <text>",
                ));
            }
            Some("clear") => self.lines.clear(),
            Some("uptime") => {
                let ticks = crate::sched::ticks();
                self.push_line(alloc::format!(
                    "up {} ticks (~{}s at 100Hz)",
                    ticks,
                    ticks / 100
                ));
            }
            Some("mem") => {
                let stats = crate::memory::pmm::stats();
                self.push_line(alloc::format!(
                    "{} MiB free / {} MiB total",
                    (stats.free_frames * 4096) / (1024 * 1024),
                    (stats.total_frames * 4096) / (1024 * 1024)
                ));
            }
            Some("echo") => {
                let rest: Vec<&str> = parts.collect();
                self.push_line(rest.join(" "));
            }
            Some(other) => self.push_line(alloc::format!("unknown command: {}", other)),
            None => {}
        }
    }

    pub fn render(&self, x: i32, y: i32, w: u32, h: u32) {
        framebuffer::with(|c| {
            c.fill_rect(x, y, w, h, (0x08, 0x08, 0x0C));

            let visible_rows = ((h as i32 - 4) / LINE_HEIGHT).max(1) as usize;
            let input_row = visible_rows.saturating_sub(1);
            let history_rows = input_row;
            let start = self.lines.len().saturating_sub(history_rows);

            let max_chars = ((w as i32 - 8) / 8).max(1) as usize;
            for (row, line) in self.lines.iter().skip(start).enumerate() {
                let text = if line.len() > max_chars {
                    &line[..max_chars]
                } else {
                    line.as_str()
                };
                c.draw_str_at(
                    x + 4,
                    y + 4 + row as i32 * LINE_HEIGHT,
                    text,
                    (0xC0, 0xC0, 0xC0),
                    None,
                );
            }

            let prompt = alloc::format!("> {}_", self.current);
            let prompt = if prompt.len() > max_chars {
                &prompt[prompt.len() - max_chars..]
            } else {
                prompt.as_str()
            };
            c.draw_str_at(
                x + 4,
                y + 4 + input_row as i32 * LINE_HEIGHT,
                prompt,
                (0x40, 0xFF, 0x40),
                None,
            );
        });
    }
}

impl Default for TerminalState {
    fn default() -> Self {
        Self::new()
    }
}
