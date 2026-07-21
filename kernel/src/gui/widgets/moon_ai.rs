//! Moon AI: the assistant, scoped down from the original ask on purpose.
//! This is keyword/pattern matching against a fixed command set, not a
//! language model -- there's no LLM embedded in this kernel (that would
//! mean a multi-gigabyte model running inference on bare metal, which is
//! categorically out of scope for a hobby OS kernel). Every command below
//! does something real (opens a real app, searches the real RAMFS, reads
//! real RTC/memory state, or triggers a real reboot/shutdown); anything
//! that doesn't match gets an honest "I don't understand that" instead of
//! a made-up answer. Free-form Q&A, code generation, and image generation
//! are explicitly not attempted -- see ROADMAP.md.

use crate::framebuffer;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const MAX_LINES: usize = 200;
const LINE_HEIGHT: i32 = 10;

pub struct MoonAiState {
    lines: VecDeque<String>,
    current: String,
}

impl MoonAiState {
    pub fn new() -> Self {
        let mut lines = VecDeque::new();
        lines.push_back(String::from(
            "Moon AI -- rule-based assistant. Type 'help' for what I can do.",
        ));
        Self {
            lines,
            current: String::new(),
        }
    }

    pub fn handle_char(&mut self, ch: u8) {
        match ch {
            b'\n' => {
                let text = core::mem::take(&mut self.current);
                self.push_line(format!("> {text}"));
                self.process(&text);
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

    /// Finds the substring following any of `markers` in `text` (e.g.
    /// "search" or "for"), trimmed -- used to pull a search term out of a
    /// loosely-phrased command like "search files for hello".
    fn word_after<'a>(text: &'a str, markers: &[&str]) -> Option<&'a str> {
        for marker in markers {
            if let Some(idx) = text.find(marker) {
                let rest = text[idx + marker.len()..].trim();
                if !rest.is_empty() {
                    return Some(rest);
                }
            }
        }
        None
    }

    fn process(&mut self, raw: &str) {
        let text = raw.to_ascii_lowercase();
        let reply = if text.contains("open settings") || text.trim() == "settings" {
            crate::gui::request_open("settings");
            String::from("Opening Settings.")
        } else if text.contains("open file") || text.contains("file manager") {
            crate::gui::request_open("files");
            String::from("Opening File Manager.")
        } else if text.contains("open terminal") || text.trim() == "terminal" {
            crate::gui::request_open("terminal");
            String::from("Opening Terminal.")
        } else if text.contains("open store") || text.contains("moon store") {
            crate::gui::request_open("store");
            String::from("Opening Moon Store.")
        } else if text.contains("open notes") || text.trim() == "notes" {
            crate::gui::request_open("notes");
            String::from("Opening Notes.")
        } else if text.contains("open calculator") || text.trim() == "calculator" {
            crate::gui::request_open("calculator");
            String::from("Opening Calculator.")
        } else if text.contains("task manager") || text.trim() == "tasks" {
            crate::gui::request_open("taskmanager");
            String::from("Opening Task Manager.")
        } else if text.contains("search") || text.contains("find") {
            let needle =
                Self::word_after(&text, &["search files for", "search for", "find", "search"])
                    .unwrap_or("")
                    .to_ascii_lowercase();
            if needle.is_empty() {
                String::from("Search what? Try: search files for <name>")
            } else {
                let root = crate::fs::root().lock();
                let matches: Vec<&str> = root
                    .list()
                    .filter(|path| path.to_ascii_lowercase().contains(&needle))
                    .collect();
                if matches.is_empty() {
                    format!("No files matching '{needle}'.")
                } else {
                    format!("Found {}: {}", matches.len(), matches.join(", "))
                }
            }
        } else if text.contains("memory") || text.contains("ram") {
            let stats = crate::memory::pmm::stats();
            format!(
                "{} MiB free / {} MiB total.",
                (stats.free_frames * 4096) / (1024 * 1024),
                (stats.total_frames * 4096) / (1024 * 1024)
            )
        } else if text.contains("time") {
            let dt = crate::drivers::rtc::read();
            format!("It's {:02}:{:02}:{:02}.", dt.hour, dt.minute, dt.second)
        } else if text.contains("date") {
            let dt = crate::drivers::rtc::read();
            format!("Today is {:04}-{:02}-{:02}.", dt.year, dt.month, dt.day)
        } else if text.contains("reboot") {
            crate::power::reboot();
        } else if text.contains("shutdown") {
            crate::power::shutdown();
        } else if text.contains("help") {
            String::from(
                "I can: open <settings|files|terminal|store|notes|calculator|task manager>, \
                 search files for <name>, tell you the time/date/memory, or reboot/shutdown.",
            )
        } else {
            String::from("I don't understand that yet. Type 'help' for what I can do.")
        };
        self.push_line(reply);
    }

    pub fn render(&self, x: i32, y: i32, w: u32, h: u32) {
        framebuffer::with(|c| {
            c.fill_rect(x, y, w, h, (0x0A, 0x0A, 0x12));

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
                    (0xC0, 0xC8, 0xE0),
                    None,
                );
            }

            let prompt = format!("ask> {}_", self.current);
            let prompt = if prompt.len() > max_chars {
                &prompt[prompt.len() - max_chars..]
            } else {
                prompt.as_str()
            };
            c.draw_str_at(
                x + 4,
                y + 4 + input_row as i32 * LINE_HEIGHT,
                prompt,
                crate::gui::theme::accent(),
                None,
            );
        });
    }
}

impl Default for MoonAiState {
    fn default() -> Self {
        Self::new()
    }
}
