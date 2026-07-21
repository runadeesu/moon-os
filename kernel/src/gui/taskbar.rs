//! A bottom taskbar listing every open window.

use super::window::Window;
use crate::framebuffer;

pub const HEIGHT: u32 = 24;

pub fn render(windows: &[Window], focused_id: Option<u32>, screen_w: usize, screen_h: usize) {
    let y = screen_h as i32 - HEIGHT as i32;

    framebuffer::with(|c| {
        c.fill_rect(0, y, screen_w as u32, HEIGHT, (0x10, 0x10, 0x16));
        c.draw_str_at(4, y + 8, "moon OS", (0x80, 0xC0, 0xFF), None);
    });

    let mut x = 90i32;
    for w in windows {
        let label_w = (w.title.len() as u32 + 2) * 8;
        let active = Some(w.id) == focused_id;
        let bg = if active {
            (0x2E, 0x5A, 0x8C)
        } else {
            (0x28, 0x28, 0x32)
        };
        framebuffer::with(|c| {
            c.fill_rect(x, y + 2, label_w, HEIGHT - 4, bg);
            c.draw_str_at(x + 8, y + 8, &w.title, (0xE0, 0xE0, 0xE0), None);
        });
        x += label_w as i32 + 8;
    }
}
