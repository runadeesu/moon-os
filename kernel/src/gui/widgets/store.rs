//! Moon Store: a local catalog of installed `.mapp` packages with a
//! one-click "launch" action per row. Honest limitation: there's no actual
//! network repository to browse yet (no networked package server exists),
//! so "the store" and "what's installed" are the same list for now --
//! this is the UI shell a real catalog/download flow would slot into
//! later, not a working store front-end.

use crate::framebuffer;
use alloc::format;
use alloc::string::String;

const ROW_H: i32 = 22;
const HEADER_H: i32 = 20;

pub struct StoreState {
    status: String,
}

impl StoreState {
    pub fn new() -> Self {
        Self {
            status: String::from("click a package to launch it"),
        }
    }

    /// `x`/`y` are local to the widget's content area, same convention as
    /// `Window::handle_click`.
    pub fn handle_click(&mut self, _x: i32, y: i32) {
        let row = (y - HEADER_H) / ROW_H;
        if row < 0 {
            return;
        }
        let packages = crate::pkg::installed();
        let Some(pkg) = packages.get(row as usize) else {
            return;
        };
        self.status = match crate::pkg::run(&pkg.file_name) {
            Ok(name) => format!("launched {}", name),
            Err(err) => format!("launch failed: {}", err),
        };
    }

    pub fn render(&self, x: i32, y: i32, w: u32, h: u32) {
        let packages = crate::pkg::installed();

        framebuffer::with(|c| {
            c.fill_rect(x, y, w, h, (0x10, 0x10, 0x16));
            c.draw_str_at(
                x + 4,
                y + 4,
                "Moon Store -- local catalog",
                (0x90, 0xC0, 0xFF),
                None,
            );

            for (row, pkg) in packages.iter().enumerate() {
                let row_y = y + HEADER_H + row as i32 * ROW_H;
                if row_y + ROW_H > y + h as i32 {
                    break;
                }
                c.fill_rect(x + 4, row_y, w - 8, ROW_H as u32 - 4, (0x18, 0x1C, 0x26));
                c.draw_str_at(
                    x + 8,
                    row_y + 6,
                    &format!("{} v{}", pkg.name, pkg.version),
                    (0xE0, 0xE0, 0xE0),
                    None,
                );
                c.draw_str_at(
                    x + w as i32 - 8 * 8 - 8,
                    row_y + 6,
                    "[Launch]",
                    (0x80, 0xE8, 0xA0),
                    None,
                );
            }
        });

        let status_y = y + h as i32 - 12;
        framebuffer::with(|c| {
            c.fill_rect(x, status_y - 2, w, 14, (0x08, 0x08, 0x0C));
            c.draw_str_at(x + 4, status_y, &self.status, (0x80, 0x80, 0x90), None);
        });
    }
}

impl Default for StoreState {
    fn default() -> Self {
        Self::new()
    }
}
