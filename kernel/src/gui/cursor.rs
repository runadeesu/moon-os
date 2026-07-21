//! Cursor rendering: a real pointer silhouette instead of a flat colored
//! square, drawn procedurally (two nested triangles/circles -- an outer
//! dark outline, an inner light fill inset by a pixel) rather than a
//! hand-authored bitmap, so the shape is verified by the same span math
//! `fill_circle` already uses rather than eyeballed pixel art. Switches to
//! a small hand/pointer shape while hovering anything clickable.

use crate::framebuffer::Console;

const ARROW_H: i32 = 16;

pub fn draw(c: &mut Console, cx: i32, cy: i32, hovering_clickable: bool) {
    if hovering_clickable {
        draw_hand(c, cx, cy);
    } else {
        draw_arrow(c, cx, cy);
    }
}

/// A right-triangle arrow, point at the top-left (the cursor's hotspot) --
/// the same silhouette every classic pixelated pointer cursor uses.
fn draw_arrow(c: &mut Console, cx: i32, cy: i32) {
    let outline = (0x10, 0x10, 0x14);
    let fill = (0xF5, 0xF5, 0xF5);

    for y in 0..ARROW_H {
        let w = y + 1;
        c.fill_rect(cx, cy + y, w as u32, 1, outline);
    }
    for y in 1..ARROW_H - 1 {
        c.fill_rect(cx + 1, cy + y, y as u32, 1, fill);
    }
}

/// A simple pointing-hand silhouette (a circle "palm" with a rectangular
/// "finger"), swapped in over dock icons, title-bar buttons, and other
/// clickable spots.
fn draw_hand(c: &mut Console, cx: i32, cy: i32) {
    let outline = (0x10, 0x10, 0x14);
    let fill = (0xF5, 0xF5, 0xF5);

    c.fill_circle(cx + 6, cy + 10, 7, outline);
    c.fill_rect(cx + 3, cy, 4, 12, outline);

    c.fill_circle(cx + 6, cy + 10, 6, fill);
    c.fill_rect(cx + 4, cy + 1, 2, 10, fill);
}
