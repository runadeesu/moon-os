//! A settings/system-info widget: live memory, scheduler, and uptime
//! stats, plus the one thing that's actually configurable so far -- the UI
//! language (`crate::i18n`), changed by clicking its row. A fuller settings
//! app (with real subsystems to configure) waits on having more of them;
//! for now this doubles as a live dashboard proving the window system can
//! host more than one kind of content.

use crate::framebuffer;
use crate::i18n::{self, Key};
use alloc::string::ToString;

const ROW_H: i32 = 12;
const HEADER_Y: i32 = 6;
const MEMORY_Y: i32 = HEADER_Y + ROW_H;
const TASKS_Y: i32 = MEMORY_Y + ROW_H;
const UPTIME_Y: i32 = TASKS_Y + ROW_H;
const LANG_Y: i32 = UPTIME_Y + ROW_H + 6;
const HINT_Y: i32 = LANG_Y + ROW_H;

const ACCENT: (u8, u8, u8) = (0x30, 0xE0, 0xFF);

pub struct SettingsState;

impl SettingsState {
    /// `x`/`y` here are local to the widget's content area (already offset
    /// past the window's title bar by the caller, `Window::handle_click`).
    /// Clicking anywhere on the language row cycles to the next language.
    pub fn handle_click(&mut self, _x: i32, y: i32) {
        if (LANG_Y - 2..LANG_Y + ROW_H).contains(&y) {
            i18n::cycle();
        }
    }

    pub fn render(&self, x: i32, y: i32, w: u32, h: u32) {
        let stats = crate::memory::pmm::stats();
        let free_mib = (stats.free_frames * 4096) / (1024 * 1024);
        let total_mib = (stats.total_frames * 4096) / (1024 * 1024);
        let ticks = crate::sched::ticks();

        framebuffer::with(|c| {
            c.fill_rect(x, y, w, h, (0x18, 0x18, 0x20));

            c.draw_str_at(x + 6, y + HEADER_Y, "moon OS -- ", (0xD0, 0xD0, 0xD0), None);
            c.draw_glyphs_at(
                x + 6 + 11 * 8,
                y + HEADER_Y,
                i18n::tr(Key::SystemInfoHeader),
                (0xD0, 0xD0, 0xD0),
                None,
            );

            let memory_label = i18n::tr(Key::MemoryLabel);
            c.draw_glyphs_at(x + 6, y + MEMORY_Y, memory_label, (0xD0, 0xD0, 0xD0), None);
            c.draw_str_at(
                x + 6 + (memory_label.len() as i32 + 1) * 8,
                y + MEMORY_Y,
                &alloc::format!(": {} / {} MiB free", free_mib, total_mib),
                (0xD0, 0xD0, 0xD0),
                None,
            );

            let tasks_label = i18n::tr(Key::TasksLabel);
            c.draw_glyphs_at(x + 6, y + TASKS_Y, tasks_label, (0xD0, 0xD0, 0xD0), None);
            c.draw_str_at(
                x + 6 + (tasks_label.len() as i32 + 1) * 8,
                y + TASKS_Y,
                &alloc::format!(": {}", crate::sched::task_count()),
                (0xD0, 0xD0, 0xD0),
                None,
            );

            let uptime_label = i18n::tr(Key::UptimeLabel);
            c.draw_glyphs_at(x + 6, y + UPTIME_Y, uptime_label, (0xD0, 0xD0, 0xD0), None);
            c.draw_str_at(
                x + 6 + (uptime_label.len() as i32 + 1) * 8,
                y + UPTIME_Y,
                &alloc::format!(": {} ticks (~{}s)", ticks, ticks / 100),
                (0xD0, 0xD0, 0xD0),
                None,
            );

            let lang_line = alloc::format!("Lang: {}", i18n::current().name()).to_string();
            c.fill_rect(
                x + 2,
                y + LANG_Y - 2,
                w - 4,
                (ROW_H + 2) as u32,
                (0x14, 0x2A, 0x36),
            );
            c.draw_str_at(x + 6, y + LANG_Y, &lang_line, ACCENT, None);

            c.draw_glyphs_at(
                x + 6,
                y + HINT_Y,
                i18n::tr(Key::LangHint),
                (0x70, 0x80, 0x90),
                None,
            );
        });
    }
}
