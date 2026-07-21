//! Notification Center: toasts anchored top-right, below the top bar, driven
//! entirely by real events elsewhere in the kernel (package install/launch,
//! file operations, power actions) -- there's no demo timer manufacturing
//! fake ones. Wi-Fi/battery/screenshot-style notifications from the original
//! wishlist don't exist because there's no real Wi-Fi/battery/screenshot
//! subsystem behind them yet; adding those toasts without real triggers
//! would be exactly the kind of fake feature this project avoids.

use crate::framebuffer;
use alloc::collections::VecDeque;
use alloc::string::String;
use spin::Mutex;

const MAX_VISIBLE: usize = 5;
/// How long a toast stays fully visible before fading, then how much longer
/// it fades before disappearing entirely (in scheduler ticks, ~100/sec).
const LIFETIME_TICKS: u64 = 500;
const FADE_TICKS: u64 = 100;
const WIDTH: u32 = 260;
const HEIGHT: i32 = 36;
const GAP: i32 = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Info,
    Success,
    Warning,
    Error,
}

struct Notification {
    kind: Kind,
    message: String,
    created_at: u64,
}

static QUEUE: Mutex<VecDeque<Notification>> = Mutex::new(VecDeque::new());

/// Records a real event as a toast. Called from the package manager, File
/// Manager, and power actions with a description of what actually happened
/// -- never with synthetic/demo content.
pub fn push(kind: Kind, message: String) {
    let mut q = QUEUE.lock();
    q.push_back(Notification {
        kind,
        message,
        created_at: crate::sched::ticks(),
    });
    while q.len() > 20 {
        q.pop_front();
    }
}

/// Renders the most recent still-live notifications stacked downward from
/// `(x_right, top)`, `x_right` being the right edge to hang them off of.
/// Also prunes anything past its lifetime -- the only place expiry happens.
pub fn render(x_right: i32, top: i32) {
    let now = crate::sched::ticks();
    let mut q = QUEUE.lock();
    q.retain(|n| now.saturating_sub(n.created_at) < LIFETIME_TICKS);

    let x = x_right - WIDTH as i32;
    let mut y = top;
    for n in q.iter().rev().take(MAX_VISIBLE) {
        let age = now.saturating_sub(n.created_at);
        let alpha = if age > LIFETIME_TICKS - FADE_TICKS {
            let fade_progress = age - (LIFETIME_TICKS - FADE_TICKS);
            (220u64.saturating_sub(220 * fade_progress / FADE_TICKS)) as u8
        } else {
            220
        };
        let color = match n.kind {
            Kind::Info => super::theme::accent(),
            Kind::Success => (0x50, 0xE8, 0x90),
            Kind::Warning => (0xE8, 0xC8, 0x40),
            Kind::Error => (0xE8, 0x58, 0x58),
        };

        framebuffer::with(|c| {
            c.glow_border(x, y, WIDTH, HEIGHT as u32, color);
            c.blend_rect(x, y, WIDTH, HEIGHT as u32, (0x0E, 0x12, 0x1E), alpha);
            c.fill_rect(x, y, 4, HEIGHT as u32, color);

            let max_chars = ((WIDTH as i32 - 20) / 8).max(1) as usize;
            let text = if n.message.len() > max_chars {
                &n.message[..max_chars]
            } else {
                n.message.as_str()
            };
            c.draw_str_at(x + 12, y + HEIGHT / 2 - 4, text, (0xE8, 0xE8, 0xE8), None);
        });
        y += HEIGHT + GAP;
    }
}
