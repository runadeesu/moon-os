//! A left-edge dock listing every open window as a clickable icon (the
//! first letter of its title), highlighting whichever one currently has
//! focus. Replaces the old bottom taskbar with the vertical dock layout the
//! desktop design calls for; click handling lives in `gui::on_mouse`, which
//! consults `icon_at` before falling back to testing window bodies.

use super::window::Window;
use crate::framebuffer;

pub const WIDTH: u32 = 56;
const ICON_SIZE: i32 = 40;
const ICON_GAP: i32 = 10;
const TOP_MARGIN: i32 = 16;
const ICON_X: i32 = 8;
const NEON: (u8, u8, u8) = (0x30, 0xE0, 0xFF);

pub fn render(windows: &[Window], focused_id: Option<u32>, screen_h: usize, top_offset: i32) {
    framebuffer::with(|c| {
        c.blend_rect(
            0,
            top_offset,
            WIDTH,
            screen_h as u32 - top_offset as u32,
            (0x0A, 0x0C, 0x18),
            215,
        );
        // Glowing right edge, matching the top bar's HUD-panel accent.
        c.blend_rect(WIDTH as i32, top_offset, 1, screen_h as u32, NEON, 130);
        c.blend_rect(WIDTH as i32 + 1, top_offset, 1, screen_h as u32, NEON, 55);
    });

    let mut y = top_offset + TOP_MARGIN;
    for w in windows {
        let active = Some(w.id) == focused_id;
        let bg = if active {
            (0x0E, 0x3A, 0x50)
        } else {
            (0x20, 0x22, 0x2E)
        };
        let letter = w.title.as_bytes().first().copied().unwrap_or(b'?');
        framebuffer::with(|c| {
            if active {
                c.glow_border(ICON_X, y, ICON_SIZE as u32, ICON_SIZE as u32, NEON);
            }
            c.fill_rect(ICON_X, y, ICON_SIZE as u32, ICON_SIZE as u32, bg);
            c.draw_char_at(
                ICON_X + (ICON_SIZE - 8) / 2,
                y + (ICON_SIZE - 8) / 2,
                letter,
                if active { NEON } else { (0xE0, 0xE0, 0xE0) },
                None,
            );
        });
        y += ICON_SIZE + ICON_GAP;
    }
}

/// Hit-tests a click against the dock's icon column; returns the index into
/// `windows` (same order `render` drew them in) whose icon was clicked.
pub fn icon_at(windows_len: usize, top_offset: i32, x: i32, y: i32) -> Option<usize> {
    if !(ICON_X..ICON_X + ICON_SIZE).contains(&x) {
        return None;
    }
    let rel = y - (top_offset + TOP_MARGIN);
    if rel < 0 {
        return None;
    }
    let stride = ICON_SIZE + ICON_GAP;
    let idx = (rel / stride) as usize;
    if rel % stride < ICON_SIZE && idx < windows_len {
        Some(idx)
    } else {
        None
    }
}
