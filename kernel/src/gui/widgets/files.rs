//! A minimal File Manager widget: lists everything in the RAMFS root, and
//! double-clicking a `.mapp` package launches it as a new ring-3 process
//! via `pkg::run` -- the first real consumer of the package format that
//! isn't a terminal command.

use crate::framebuffer;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const ROW_H: i32 = 16;
const HEADER_H: i32 = 20;
/// How close together (in scheduler ticks, ~100/sec) two clicks on the same
/// row need to land to count as a double-click.
const DOUBLE_CLICK_TICKS: u64 = 40;

pub struct FileManagerState {
    last_click: Option<(usize, u64)>,
    status: String,
}

impl FileManagerState {
    pub fn new() -> Self {
        Self {
            last_click: None,
            status: String::from("double-click a .mapp package to launch it"),
        }
    }

    fn entries() -> Vec<String> {
        let root = crate::fs::root().lock();
        let mut names: Vec<String> = root.list().map(String::from).collect();
        names.sort();
        names
    }

    /// `x`/`y` are local to the widget's content area, same convention as
    /// `Window::handle_click`.
    pub fn handle_click(&mut self, _x: i32, y: i32) {
        let row = (y - HEADER_H) / ROW_H;
        if row < 0 {
            return;
        }
        let row = row as usize;
        let entries = Self::entries();
        let Some(name) = entries.get(row) else {
            return;
        };

        let now = crate::sched::ticks();
        let is_double_click = matches!(
            self.last_click,
            Some((last_row, last_tick))
                if last_row == row && now.saturating_sub(last_tick) < DOUBLE_CLICK_TICKS
        );
        self.last_click = Some((row, now));

        if !is_double_click {
            self.status = format!("selected: {}", name);
            return;
        }

        if !name.ends_with(".mapp") {
            self.status = format!("{} is not a launchable package", name);
            return;
        }
        self.status = match crate::pkg::run(name) {
            Ok(pkg_name) => format!("launched {}", pkg_name),
            Err(err) => format!("launch failed: {}", err),
        };
    }

    pub fn render(&self, x: i32, y: i32, w: u32, h: u32) {
        let entries = Self::entries();

        framebuffer::with(|c| {
            c.fill_rect(x, y, w, h, (0x10, 0x10, 0x16));
            c.draw_str_at(x + 4, y + 4, "/  (RAMFS root)", (0x90, 0xC0, 0xFF), None);

            let max_chars = ((w as i32 - 8) / 8).max(1) as usize;
            for (row, name) in entries.iter().enumerate() {
                let row_y = y + HEADER_H + row as i32 * ROW_H;
                if row_y + ROW_H > y + h as i32 {
                    break;
                }
                let is_pkg = name.ends_with(".mapp");
                let color = if is_pkg {
                    (0x80, 0xE8, 0xA0)
                } else {
                    (0xC0, 0xC0, 0xC0)
                };
                let text = if name.len() > max_chars {
                    &name[..max_chars]
                } else {
                    name.as_str()
                };
                c.draw_str_at(x + 8, row_y, text, color, None);
            }
        });

        let status_y = y + h as i32 - 12;
        framebuffer::with(|c| {
            c.fill_rect(x, status_y - 2, w, 14, (0x08, 0x08, 0x0C));
            c.draw_str_at(x + 4, status_y, &self.status, (0x80, 0x80, 0x90), None);
        });
    }
}

impl Default for FileManagerState {
    fn default() -> Self {
        Self::new()
    }
}
