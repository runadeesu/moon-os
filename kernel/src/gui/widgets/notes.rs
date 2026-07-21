//! A plain-text notes app, auto-saved to a real RAMFS file (`/notes.txt`)
//! on every keystroke -- doubles as the "ToDo list" from the original
//! wishlist (write a `- [ ] task` per line) since there's no separate
//! interactive desktop-widget framework for that, but a real editable text
//! buffer covers the same need honestly. No rich text, no multiple notes,
//! no syntax highlighting -- a single persisted plain-text buffer.

use crate::framebuffer;
use alloc::string::String;
use alloc::vec::Vec;

const NOTES_PATH: &str = "/notes.txt";
const LINE_HEIGHT: i32 = 12;

pub struct NotesState {
    content: String,
}

impl NotesState {
    pub fn new() -> Self {
        let content = crate::fs::root()
            .lock()
            .read(NOTES_PATH)
            .and_then(|data| core::str::from_utf8(data).ok())
            .map(String::from)
            .unwrap_or_default();
        Self { content }
    }

    fn save(&self) {
        crate::fs::root()
            .lock()
            .write(NOTES_PATH, self.content.as_bytes());
    }

    pub fn handle_char(&mut self, ch: u8) {
        match ch {
            b'\n' => self.content.push('\n'),
            0x08 => {
                self.content.pop();
            }
            0x20..=0x7E => self.content.push(ch as char),
            _ => return,
        }
        self.save();
    }

    pub fn render(&self, x: i32, y: i32, w: u32, h: u32) {
        framebuffer::with(|c| {
            c.fill_rect(x, y, w, h, (0x14, 0x14, 0x1A));
            c.draw_str_at(
                x + 4,
                y + 4,
                "Notes -- autosaved to /notes.txt",
                (0x90, 0xC0, 0xFF),
                None,
            );

            let max_chars = ((w as i32 - 8) / 8).max(1) as usize;
            let visible_rows = ((h as i32 - 22) / LINE_HEIGHT).max(1) as usize;
            let lines: Vec<&str> = self.content.split('\n').collect();
            let start = lines.len().saturating_sub(visible_rows);
            for (row, line) in lines.iter().skip(start).enumerate() {
                let text = if line.len() > max_chars {
                    &line[..max_chars]
                } else {
                    line
                };
                c.draw_str_at(
                    x + 4,
                    y + 18 + row as i32 * LINE_HEIGHT,
                    text,
                    (0xD8, 0xD8, 0xD8),
                    None,
                );
            }
        });
    }
}

impl Default for NotesState {
    fn default() -> Self {
        Self::new()
    }
}
