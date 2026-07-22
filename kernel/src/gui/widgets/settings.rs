//! A settings/system-info widget: live memory, scheduler, and uptime
//! stats, plus what's actually configurable -- UI language (`crate::i18n`),
//! the accent color (`super::super::theme`), mouse sensitivity
//! (`super::super::mouse_speed`), a real network status line (actual
//! DHCP-leased IP or "no lease", `crate::net`), Do Not Disturb for
//! notification toasts (`super::super::notifications`), and real power
//! actions (reboot/shutdown, `crate::power`) -- each changed/triggered by
//! clicking its row. Bluetooth/battery/account panels still aren't here:
//! `crate::drivers::bluetooth`/`crate::power::battery_status` only ever
//! have one honest answer to report ("not detected"/"no battery"), and
//! there's no real account system beyond the login screen's `/etc/passwd`
//! yet -- a settings row for either would just be decoration.

use crate::framebuffer;
use crate::i18n::{self, Key};
use alloc::string::ToString;

const ROW_H: i32 = 12;
const HEADER_Y: i32 = 6;
const MEMORY_Y: i32 = HEADER_Y + ROW_H;
const TASKS_Y: i32 = MEMORY_Y + ROW_H;
const UPTIME_Y: i32 = TASKS_Y + ROW_H;
const LANG_Y: i32 = UPTIME_Y + ROW_H + 6;
const ACCENT_Y: i32 = LANG_Y + ROW_H;
const MOUSE_Y: i32 = ACCENT_Y + ROW_H;
const NETWORK_Y: i32 = MOUSE_Y + ROW_H;
const DND_Y: i32 = NETWORK_Y + ROW_H;
const HINT_Y: i32 = DND_Y + ROW_H;
const POWER_Y: i32 = HINT_Y + ROW_H + 6;

pub struct SettingsState;

impl SettingsState {
    /// `x`/`y` here are local to the widget's content area (already offset
    /// past the window's title bar by the caller, `Window::handle_click`).
    /// Clicking the language row cycles the UI language, the accent row
    /// cycles the accent color, and the power row's two halves trigger a
    /// real reboot/shutdown.
    pub fn handle_click(&mut self, x: i32, y: i32) {
        if (LANG_Y - 2..LANG_Y + ROW_H).contains(&y) {
            i18n::cycle();
        } else if (ACCENT_Y - 2..ACCENT_Y + ROW_H).contains(&y) {
            super::super::theme::cycle_accent();
        } else if (MOUSE_Y - 2..MOUSE_Y + ROW_H).contains(&y) {
            super::super::mouse_speed::cycle();
        } else if (DND_Y - 2..DND_Y + ROW_H).contains(&y) {
            super::super::notifications::toggle_dnd();
        } else if (POWER_Y - 2..POWER_Y + ROW_H).contains(&y) {
            if x < 90 {
                crate::power::reboot();
            } else {
                crate::power::shutdown();
            }
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

            let accent = crate::gui::theme::accent();
            let lang_line = alloc::format!("Lang: {}", i18n::current().name()).to_string();
            c.fill_rect(
                x + 2,
                y + LANG_Y - 2,
                w - 4,
                (ROW_H + 2) as u32,
                (0x14, 0x2A, 0x36),
            );
            c.draw_str_at(x + 6, y + LANG_Y, &lang_line, accent, None);

            let accent_line = alloc::format!("Accent: {}", crate::gui::theme::accent_name());
            c.fill_rect(
                x + 2,
                y + ACCENT_Y - 2,
                w - 4,
                (ROW_H + 2) as u32,
                (0x14, 0x2A, 0x36),
            );
            c.draw_str_at(x + 6, y + ACCENT_Y, &accent_line, accent, None);

            let mouse_line = alloc::format!("Mouse speed: {}", super::super::mouse_speed::name());
            c.fill_rect(
                x + 2,
                y + MOUSE_Y - 2,
                w - 4,
                (ROW_H + 2) as u32,
                (0x14, 0x2A, 0x36),
            );
            c.draw_str_at(x + 6, y + MOUSE_Y, &mouse_line, accent, None);

            let network_line = if crate::net::is_up() {
                let ip = crate::net::our_ip();
                if ip == crate::net::UNSPECIFIED_IP {
                    alloc::string::String::from("Network: no lease")
                } else {
                    alloc::format!("Network: {}", crate::net::format_ip(ip))
                }
            } else {
                alloc::string::String::from("Network: no NIC")
            };
            c.draw_str_at(
                x + 6,
                y + NETWORK_Y,
                &network_line,
                (0x90, 0x94, 0xA0),
                None,
            );

            let dnd_on = super::super::notifications::dnd_enabled();
            let dnd_line = alloc::format!("Do Not Disturb: {}", if dnd_on { "On" } else { "Off" });
            c.fill_rect(
                x + 2,
                y + DND_Y - 2,
                w - 4,
                (ROW_H + 2) as u32,
                (0x14, 0x2A, 0x36),
            );
            c.draw_str_at(x + 6, y + DND_Y, &dnd_line, accent, None);

            c.draw_glyphs_at(
                x + 6,
                y + HINT_Y,
                i18n::tr(Key::LangHint),
                (0x70, 0x80, 0x90),
                None,
            );

            c.fill_rect(
                x + 2,
                y + POWER_Y - 2,
                84,
                (ROW_H + 2) as u32,
                (0x3A, 0x1A, 0x1A),
            );
            c.draw_str_at(x + 6, y + POWER_Y, "Reboot", (0xE8, 0xA0, 0xA0), None);
            c.fill_rect(
                x + 92,
                y + POWER_Y - 2,
                84,
                (ROW_H + 2) as u32,
                (0x3A, 0x1A, 0x1A),
            );
            c.draw_str_at(x + 96, y + POWER_Y, "Shutdown", (0xE8, 0xA0, 0xA0), None);
        });
    }
}
