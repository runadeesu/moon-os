//! Top status bar: a small crescent-moon logo, the "moon OS" wordmark, a
//! live clock/date pulled from the CMOS RTC in the center, and a network
//! status readout on the right.

use crate::framebuffer;

pub const HEIGHT: u32 = 26;
const NEON: (u8, u8, u8) = (0x30, 0xE0, 0xFF);

pub fn render(screen_w: usize) {
    let bg = (0x0A, 0x0C, 0x18);

    framebuffer::with(|c| {
        c.blend_rect(0, 0, screen_w as u32, HEIGHT, bg, 215);
        // A thin glowing edge along the bottom -- the HUD-panel look
        // carries through the top bar, dock, and desktop widget panels.
        c.blend_rect(0, HEIGHT as i32, screen_w as u32, 1, NEON, 130);
        c.blend_rect(0, HEIGHT as i32 + 1, screen_w as u32, 1, NEON, 55);

        // Crescent logo: a bright disc with a smaller disc punched out in
        // the bar's own (flat) background color.
        let cy = HEIGHT as i32 / 2;
        c.fill_circle(16, cy, 8, (0xC8, 0xD8, 0xFF));
        c.fill_circle(20, cy - 3, 7, bg);

        c.draw_str_at(32, 9, "moon OS", (0xB0, 0xD0, 0xFF), None);
    });

    let dt = crate::drivers::rtc::read();
    let clock = alloc::format!(
        "{:04}-{:02}-{:02}  {:02}:{:02}:{:02}",
        dt.year,
        dt.month,
        dt.day,
        dt.hour,
        dt.minute,
        dt.second
    );
    let clock_x = (screen_w as i32 / 2) - (clock.len() as i32 * 8) / 2;

    let net_status = crate::net::status_summary();
    let net_x = screen_w as i32 - (net_status.len() as i32 * 8) - 10;

    framebuffer::with(|c| {
        c.draw_str_at(clock_x, 9, &clock, (0xE0, 0xE0, 0xE0), None);
        c.draw_str_at(net_x, 9, &net_status, (0x90, 0xE0, 0xA8), None);
    });
}
