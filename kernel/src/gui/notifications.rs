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

/// Which real subsystem raised the notification -- lets the bell dropdown
/// filter by source. Not a cosmetic tag: every call site below is an actual
/// event from that actual subsystem, same honesty rule as the rest of this
/// module.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Files,
    Packages,
    System,
}

impl Category {
    pub const ALL: [Category; 3] = [Category::Files, Category::Packages, Category::System];

    pub fn label(self) -> &'static str {
        match self {
            Category::Files => "Files",
            Category::Packages => "Packages",
            Category::System => "System",
        }
    }
}

#[derive(Clone)]
struct Notification {
    kind: Kind,
    category: Category,
    message: String,
    created_at: u64,
}

static QUEUE: Mutex<VecDeque<Notification>> = Mutex::new(VecDeque::new());
/// Every notification ever pushed (capped), independent of the fading toast
/// queue above -- backs the taskbar bell's dropdown and unread badge.
static HISTORY: Mutex<VecDeque<Notification>> = Mutex::new(VecDeque::new());
static UNREAD: AtomicUsize = AtomicUsize::new(0);
/// `None` = show every category. Cycled by clicking the dropdown's filter
/// row; a real filter over the categories actually in `HISTORY`.
static CATEGORY_FILTER: Mutex<Option<Category>> = Mutex::new(None);
/// Do Not Disturb, toggled from Settings: suppresses the fading toast
/// pop-ups while still recording everything to `HISTORY` (so nothing is
/// silently lost, it just doesn't interrupt on-screen) -- a real behavior
/// change, not a cosmetic switch.
static DND: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn dnd_enabled() -> bool {
    DND.load(Ordering::Relaxed)
}

pub fn toggle_dnd() {
    DND.fetch_xor(true, Ordering::Relaxed);
}

pub fn cycle_category_filter() {
    let mut f = CATEGORY_FILTER.lock();
    *f = match *f {
        None => Some(Category::ALL[0]),
        Some(c) => {
            let idx = Category::ALL.iter().position(|&x| x == c).unwrap_or(0);
            if idx + 1 < Category::ALL.len() {
                Some(Category::ALL[idx + 1])
            } else {
                None
            }
        }
    };
}

pub fn category_filter() -> Option<Category> {
    *CATEGORY_FILTER.lock()
}

/// Records a real event as a toast. Called from the package manager, File
/// Manager, and power actions with a description of what actually happened
/// -- never with synthetic/demo content.
pub fn push(kind: Kind, category: Category, message: String) {
    let now = crate::sched::ticks();
    if !dnd_enabled() {
        let mut q = QUEUE.lock();
        q.push_back(Notification {
            kind,
            category,
            message: message.clone(),
            created_at: now,
        });
        while q.len() > 20 {
            q.pop_front();
        }
    }

    let mut h = HISTORY.lock();
    h.push_back(Notification {
        kind,
        category,
        message,
        created_at: now,
    });
    while h.len() > HISTORY_CAP {
        h.pop_front();
    }
    UNREAD.fetch_add(1, Ordering::Relaxed);
}

/// Drops every past notification -- a real "Clear all", not a cosmetic
/// button that leaves the list untouched.
pub fn clear_history() {
    HISTORY.lock().clear();
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
    pub category: Category,
    pub message: String,
    pub age_secs: u64,
}

/// A snapshot of past notifications, most recent first, for the taskbar
/// bell's dropdown panel -- respects `category_filter()`.
pub fn history() -> Vec<HistoryEntry> {
    let now = crate::sched::ticks();
    let filter = category_filter();
    HISTORY
        .lock()
        .iter()
        .rev()
        .filter(|n| filter.is_none_or(|f| n.category == f))
        .map(|n| HistoryEntry {
            kind: n.kind,
            category: n.category,
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
            c.frosted_glass_rect(x, y, WIDTH, HEIGHT as u32, (0x0E, 0x12, 0x1E), alpha);
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
/// The extra header row holding the category filter + "Clear all" -- always
/// present, even with zero notifications, so both are reachable regardless.
const HEADER_ROW_H: i32 = 18;

/// Total panel height for the current entry count -- shared by `render_*`
/// and the taskbar's click hit-testing so they never disagree about where
/// the panel actually is.
pub fn panel_height() -> i32 {
    let rows = history().len().clamp(1, HISTORY_MAX_ROWS);
    HEADER_ROW_H + rows as i32 * HISTORY_ROW_H
}

pub fn panel_width() -> i32 {
    HISTORY_PANEL_W as i32
}

/// `true` if `(x, y)` (already relative to the panel's top-left corner)
/// landed on the header's filter half (left) vs. clear-all half (right).
/// `None` if the click missed the header row entirely.
pub fn header_hit(local_x: i32, local_y: i32) -> Option<bool> {
    if !(0..HEADER_ROW_H).contains(&local_y) {
        return None;
    }
    Some(local_x < HISTORY_PANEL_W as i32 / 2)
}

/// The taskbar bell's dropdown: every notification still in `HISTORY`
/// (unlike the fading toast queue, this doesn't expire), opening upward
/// from `(x_right, bottom)` since the taskbar lives at the bottom of the
/// screen.
pub fn render_history_panel(x_right: i32, bottom: i32) {
    let entries = history();
    let rows = entries.len().clamp(1, HISTORY_MAX_ROWS);
    let h = HEADER_ROW_H + rows as i32 * HISTORY_ROW_H;
    let x = x_right - HISTORY_PANEL_W as i32;
    let y = bottom - h;
    let neon = super::theme::accent();

    framebuffer::with(|c| {
        c.glow_border(x, y, HISTORY_PANEL_W, h as u32, neon);
        c.frosted_glass_rect(x, y, HISTORY_PANEL_W, h as u32, (0x12, 0x16, 0x22), 215);

        let filter_label = match category_filter() {
            None => alloc::string::String::from("Category: All"),
            Some(c) => alloc::format!("Category: {}", c.label()),
        };
        c.draw_str_at(x + 6, y + 4, &filter_label, neon, None);
        c.draw_str_at(
            x + HISTORY_PANEL_W as i32 - 46,
            y + 4,
            "[Clear]",
            (0xE8, 0x90, 0x90),
            None,
        );
        c.fill_rect(
            x,
            y + HEADER_ROW_H - 1,
            HISTORY_PANEL_W,
            1,
            (0x30, 0x34, 0x40),
        );

        let list_y = y + HEADER_ROW_H;
        if entries.is_empty() {
            c.draw_str_at(
                x + 8,
                list_y + 5,
                "No notifications yet",
                (0x80, 0x84, 0x90),
                None,
            );
            return;
        }

        let max_chars = ((HISTORY_PANEL_W as i32 - 60) / 8).max(1) as usize;
        for (i, entry) in entries.iter().take(HISTORY_MAX_ROWS).enumerate() {
            let row_y = list_y + i as i32 * HISTORY_ROW_H;
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
