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
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

const MAX_VISIBLE: usize = 5;
/// How long a toast stays fully visible before fading, then how much longer
/// it fades before disappearing entirely (in scheduler ticks, ~100/sec).
const LIFETIME_TICKS: u64 = 500;
const FADE_TICKS: u64 = 100;
const WIDTH: u32 = 260;
const HEIGHT: i32 = 36;
const GAP: i32 = 8;
/// How many past notifications the taskbar bell's dropdown keeps, regardless
/// of whether their toast already faded.
const HISTORY_CAP: usize = 50;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone)]
struct Notification {
    kind: Kind,
    message: String,
    created_at: u64,
}

static QUEUE: Mutex<VecDeque<Notification>> = Mutex::new(VecDeque::new());
/// Every notification ever pushed (capped), independent of the fading toast
/// queue above -- backs the taskbar bell's dropdown and unread badge.
static HISTORY: Mutex<VecDeque<Notification>> = Mutex::new(VecDeque::new());
static UNREAD: AtomicUsize = AtomicUsize::new(0);

/// Records a real event as a toast. Called from the package manager, File
/// Manager, and power actions with a description of what actually happened
/// -- never with synthetic/demo content.
pub fn push(kind: Kind, message: String) {
    let now = crate::sched::ticks();
    let mut q = QUEUE.lock();
    q.push_back(Notification {
        kind,
        message: message.clone(),
        created_at: now,
    });
    while q.len() > 20 {
        q.pop_front();
    }
    drop(q);

    let mut h = HISTORY.lock();
    h.push_back(Notification {
        kind,
        message,
        created_at: now,
    });
    while h.len() > HISTORY_CAP {
        h.pop_front();
    }
    UNREAD.fetch_add(1, Ordering::Relaxed);
}

/// Notifications the user hasn't opened the bell dropdown to see yet -- a
/// real count, not a decorative badge.
pub fn unread_count() -> usize {
    UNREAD.load(Ordering::Relaxed)
}

/// Called when the taskbar bell dropdown is opened.
pub fn mark_all_read() {
    UNREAD.store(0, Ordering::Relaxed);
}

pub struct HistoryEntry {
    pub kind: Kind,
    pub message: String,
    pub age_secs: u64,
}

/// A snapshot of past notifications, most recent first, for the taskbar
/// bell's dropdown panel.
pub fn history() -> Vec<HistoryEntry> {
    let now = crate::sched::ticks();
    HISTORY
        .lock()
        .iter()
        .rev()
        .map(|n| HistoryEntry {
            kind: n.kind,
            message: n.message.clone(),
            age_secs: now.saturating_sub(n.created_at) / 100,
        })
        .collect()
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

const HISTORY_ROW_H: i32 = 18;
const HISTORY_PANEL_W: u32 = 280;
const HISTORY_MAX_ROWS: usize = 10;

/// The taskbar bell's dropdown: every notification still in `HISTORY`
/// (unlike the fading toast queue, this doesn't expire), opening upward
/// from `(x_right, bottom)` since the taskbar lives at the bottom of the
/// screen.
pub fn render_history_panel(x_right: i32, bottom: i32) {
    let entries = history();
    let rows = entries.len().clamp(1, HISTORY_MAX_ROWS);
    let h = rows as i32 * HISTORY_ROW_H;
    let x = x_right - HISTORY_PANEL_W as i32;
    let y = bottom - h;
    let neon = super::theme::accent();

    framebuffer::with(|c| {
        c.glow_border(x, y, HISTORY_PANEL_W, h as u32, neon);
        c.blend_rect(x, y, HISTORY_PANEL_W, h as u32, (0x12, 0x16, 0x22), 240);

        if entries.is_empty() {
            c.draw_str_at(
                x + 8,
                y + 5,
                "No notifications yet",
                (0x80, 0x84, 0x90),
                None,
            );
            return;
        }

        let max_chars = ((HISTORY_PANEL_W as i32 - 60) / 8).max(1) as usize;
        for (i, entry) in entries.iter().take(HISTORY_MAX_ROWS).enumerate() {
            let row_y = y + i as i32 * HISTORY_ROW_H;
            let color = match entry.kind {
                Kind::Info => neon,
                Kind::Success => (0x50, 0xE8, 0x90),
                Kind::Warning => (0xE8, 0xC8, 0x40),
                Kind::Error => (0xE8, 0x58, 0x58),
            };
            c.fill_rect(x + 4, row_y + 4, 4, 10, color);
            let text = if entry.message.len() > max_chars {
                &entry.message[..max_chars]
            } else {
                entry.message.as_str()
            };
            c.draw_str_at(x + 14, row_y + 5, text, (0xD8, 0xD8, 0xD8), None);
            c.draw_str_at(
                x + HISTORY_PANEL_W as i32 - 44,
                row_y + 5,
                &alloc::format!("{}s", entry.age_secs),
                (0x70, 0x74, 0x80),
                None,
            );
        }
    });
}
