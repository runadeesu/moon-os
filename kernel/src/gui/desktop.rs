//! Procedural desktop wallpaper: a night-sky gradient, a starfield, a big
//! moon, and a layered mountain silhouette -- all drawn from primitives
//! (gradient scanlines, filled circles, per-column height fills), since
//! there's no PNG/image decoding in the kernel yet. Matches the "dark,
//! moon/space themed desktop" look the project is going for.

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
/// the same list every frame rather than re-rolling it.
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

pub fn render(stars: &[Star], screen_w: usize, screen_h: usize) {
    let sw = screen_w as i32;
    let sh = screen_h as i32;

    let top = (0x05, 0x06, 0x12);
    let bottom = (0x1E, 0x2A, 0x46);
    framebuffer::with(|c| {
        for y in 0..sh {
            let color = (
                lerp(top.0, bottom.0, y, sh),
                lerp(top.1, bottom.1, y, sh),
                lerp(top.2, bottom.2, y, sh),
            );
            c.fill_rect(0, y, screen_w as u32, 1, color);
        }
    });

    // The moon: a large, mostly-visible disc sitting in the upper-middle of
    // the sky, plus a few darker "crater" circles for texture.
    let moon_r = sh * 3 / 10;
    let moon_cx = sw / 2;
    let moon_cy = sh * 3 / 10;
    framebuffer::with(|c| {
        c.fill_circle(moon_cx, moon_cy, moon_r, (0xD8, 0xDC, 0xEA));
        c.fill_circle(
            moon_cx - moon_r / 3,
            moon_cy - moon_r / 4,
            moon_r / 6,
            (0xC0, 0xC4, 0xD6),
        );
        c.fill_circle(
            moon_cx + moon_r / 4,
            moon_cy + moon_r / 5,
            moon_r / 8,
            (0xC6, 0xCA, 0xDC),
        );
        c.fill_circle(
            moon_cx - moon_r / 8,
            moon_cy + moon_r / 3,
            moon_r / 10,
            (0xC6, 0xCA, 0xDC),
        );
    });

    framebuffer::with(|c| {
        for star in stars {
            let b = star.brightness;
            c.fill_rect(
                star.x,
                star.y,
                1,
                1,
                (b, b, (b as u16 + 0x18).min(0xFF) as u8),
            );
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
