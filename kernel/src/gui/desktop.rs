//! Procedural, *animated* desktop wallpaper: a day/night sky gradient
//! driven by the real CMOS RTC clock (not just a static night scene
//! anymore), a sun or moon that actually moves across the sky over the
//! course of the day, drifting parallax clouds, and a twinkling starfield
//! that fades in and out with the night. All drawn from primitives
//! (gradient scanlines, filled circles, per-column height fills) since
//! there's no PNG/image decoding in the kernel -- and all integer math, no
//! floats: nothing in this kernel saves/restores FPU/SSE state across a
//! context switch yet, so introducing float usage anywhere would risk
//! silent corruption the moment two tasks both touch it.

use crate::framebuffer;
use alloc::vec::Vec;

pub struct Star {
    x: i32,
    y: i32,
    brightness: u8,
}

/// A tiny xorshift PRNG -- no `rand` crate in a `no_std` kernel, and all we
/// need is a fixed, deterministic starfield generated once at boot.
struct Rng(u32);

impl Rng {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
}

/// Generates a fixed starfield once at GUI init time; `render` just redraws
/// the same list every frame (fading them by time of day) rather than
/// re-rolling it.
pub fn generate_stars(screen_w: usize, screen_h: usize, count: usize) -> Vec<Star> {
    let mut rng = Rng(0x9E3779B9);
    let mut stars = Vec::with_capacity(count);
    let sky_h = (screen_h * 2 / 3).max(1);
    for _ in 0..count {
        let x = (rng.next_u32() as usize % screen_w) as i32;
        let y = (rng.next_u32() as usize % sky_h) as i32;
        let brightness = 0x60 + (rng.next_u32() % 0xA0) as u8;
        stars.push(Star { x, y, brightness });
    }
    stars
}

/// Deterministic "skyline" height for the mountain silhouette at column `x`:
/// two overlapping triangle-wave ridges standing in for a real heightmap.
fn ridge_height(x: i32, offset: i32, period: i32, amp: i32) -> i32 {
    let m = (x + offset).rem_euclid(period);
    let half = period / 2;
    let v = if m < half { m } else { period - m };
    (v * amp) / half.max(1)
}

fn lerp(a: u8, b: u8, t: i32, max: i32) -> u8 {
    let a = i32::from(a);
    let b = i32::from(b);
    (a + (b - a) * t / max.max(1)) as u8
}

fn lerp_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: i32, max: i32) -> (u8, u8, u8) {
    (
        lerp(a.0, b.0, t, max),
        lerp(a.1, b.1, t, max),
        lerp(a.2, b.2, t, max),
    )
}

/// Piecewise-linear "how deep into night are we" factor, 0 (full day) to
/// 256 (full night), as a function of minutes-since-midnight -- real dawn
/// (5:00-8:00) and dusk (17:00-21:00) transitions, not a hard day/night cut.
fn night_factor(minute_of_day: i32) -> i32 {
    const NIGHT: i32 = 256;
    const DAY: i32 = 0;
    // (minute, factor) breakpoints, in order.
    let points = [
        (0, NIGHT),
        (5 * 60, NIGHT),
        (8 * 60, DAY),
        (17 * 60, DAY),
        (21 * 60, NIGHT),
        (24 * 60, NIGHT),
    ];
    for pair in points.windows(2) {
        let (m0, f0) = pair[0];
        let (m1, f1) = pair[1];
        if minute_of_day >= m0 && minute_of_day <= m1 {
            let span = (m1 - m0).max(1);
            return f0 + (f1 - f0) * (minute_of_day - m0) / span;
        }
    }
    NIGHT
}

/// A soft round cloud: three overlapping circles, drifting slowly to the
/// right and wrapping around. Tinted darker at night than during the day.
fn draw_cloud(c: &mut framebuffer::Console, x: i32, y: i32, scale: i32, color: (u8, u8, u8)) {
    let r = scale;
    for (dx, dy, dr) in [(0, 0, r), (r, r / 3, r * 3 / 4), (-r, r / 4, r * 2 / 3)] {
        c.fill_circle(x + dx, y + dy, dr.max(1), color);
    }
}

pub fn render(stars: &[Star], screen_w: usize, screen_h: usize, ticks: u64) {
    let sw = screen_w as i32;
    let sh = screen_h as i32;

    let dt = crate::drivers::rtc::read();
    let minute_of_day = i32::from(dt.hour) * 60 + i32::from(dt.minute);
    let night = night_factor(minute_of_day); // 0 (day) .. 256 (night)

    let night_top = (0x05, 0x06, 0x12);
    let night_bottom = (0x1E, 0x2A, 0x46);
    let day_top = (0x2E, 0x6E, 0xC8);
    let day_bottom = (0x9E, 0xD4, 0xF0);
    let top = lerp_rgb(day_top, night_top, night, 256);
    let bottom = lerp_rgb(day_bottom, night_bottom, night, 256);

    framebuffer::with(|c| {
        for y in 0..sh {
            let color = lerp_rgb(top, bottom, y, sh);
            c.fill_rect(0, y, screen_w as u32, 1, color);
        }
    });

    // The sun/moon sweeps left-to-right across the sky over its half of the
    // day (sun: roughly 6:00-18:00, moon: 18:00-6:00) rather than sitting
    // in one fixed spot.
    let (body_progress, is_moon) = if night >= 128 {
        // Night half: 21:00 -> 24:00 -> 5:00 maps to 0..1 (scaled by 1000).
        let night_start = 21 * 60;
        let night_len = 24 * 60 - 21 * 60 + 5 * 60;
        let elapsed = (minute_of_day - night_start).rem_euclid(24 * 60);
        (elapsed * 1000 / night_len, true)
    } else {
        let day_start = 5 * 60;
        let day_len = 21 * 60 - 5 * 60;
        let elapsed = (minute_of_day - day_start).clamp(0, day_len);
        (elapsed * 1000 / day_len, false)
    };
    let body_r = sh * 3 / 20;
    let body_cx = sw / 8 + (sw * 3 / 4) * body_progress / 1000;
    // Arcs across the sky: low near rise/set (progress 0 or 1000), highest
    // at the midpoint of its traverse (progress 500) -- a real path, not a
    // fixed height.
    let arc = 500 - (body_progress - 500).abs(); // 0..500, peaks at the midpoint
    let body_cy = sh * 3 / 5 - (sh * 2 / 5) * arc / 500;

    framebuffer::with(|c| {
        if is_moon {
            c.fill_circle(body_cx, body_cy, body_r, (0xD8, 0xDC, 0xEA));
            c.fill_circle(
                body_cx - body_r / 3,
                body_cy - body_r / 4,
                body_r / 6,
                (0xC0, 0xC4, 0xD6),
            );
            c.fill_circle(
                body_cx + body_r / 4,
                body_cy + body_r / 5,
                body_r / 8,
                (0xC6, 0xCA, 0xDC),
            );
            c.fill_circle(
                body_cx - body_r / 8,
                body_cy + body_r / 3,
                body_r / 10,
                (0xC6, 0xCA, 0xDC),
            );
        } else {
            c.glow_border(
                body_cx - body_r,
                body_cy - body_r,
                body_r as u32 * 2,
                body_r as u32 * 2,
                (0xFF, 0xE0, 0x90),
            );
            c.fill_circle(body_cx, body_cy, body_r, (0xFF, 0xE8, 0xA0));
        }
    });

    if night > 40 {
        let star_alpha = ((night - 40) * 255 / 216).clamp(0, 255) as u8;
        framebuffer::with(|c| {
            for star in stars {
                let b = (u16::from(star.brightness) * u16::from(star_alpha) / 255) as u8;
                c.fill_rect(
                    star.x,
                    star.y,
                    1,
                    1,
                    (b, b, (b as u16 + 0x18).min(0xFF) as u8),
                );
            }
        });
    }

    // Drifting clouds: slow horizontal parallax, two layers at different
    // speeds/heights/scales, wrapping around the screen width.
    let cloud_color = lerp_rgb((0xF0, 0xF0, 0xF5), (0x30, 0x38, 0x48), night, 256);
    framebuffer::with(|c| {
        for i in 0..4 {
            let speed = 6 + i * 3; // pixels per 100 ticks-ish, via the modulo below
            let base_x = (sw / 4) * i;
            let drift = ((ticks / 4) as i32 * speed / 10) % (sw + 200);
            let x = (base_x + drift).rem_euclid(sw + 200) - 100;
            let y = sh / 12 + (i % 2) * sh / 14;
            draw_cloud(c, x, y, 14 + (i % 2) * 6, cloud_color);
        }
    });

    // Two layers of mountains, far (lighter, taller) then near (darker),
    // each a per-column vertical fill up from the deterministic ridge line.
    framebuffer::with(|c| {
        for x in 0..sw {
            let h = 60 + ridge_height(x, sw / 3, 320, 90);
            c.fill_rect(x, sh - h, 1, h as u32, (0x14, 0x18, 0x28));
        }
        for x in 0..sw {
            let h = 30 + ridge_height(x, sw / 7, 180, 60);
            c.fill_rect(x, sh - h, 1, h as u32, (0x08, 0x0A, 0x14));
        }
    });

    // A faint cyan scanline pattern over everything -- a cheap, very subtle
    // CRT/HUD texture that reads as "futuristic" without repainting the
    // whole scene at a different color.
    framebuffer::with(|c| {
        let mut sy = 0;
        while sy < sh {
            c.blend_rect(0, sy, screen_w as u32, 1, (0x40, 0xE0, 0xFF), 10);
            sy += 3;
        }
    });
}
