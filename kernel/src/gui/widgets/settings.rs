//! A read-only settings/system-info widget: live memory, scheduler, and
//! uptime stats. A real settings app (with anything to configure) waits on
//! having actual configurable subsystems; for now this doubles as a simple
//! live dashboard proving the window system can host more than one kind of
//! content.

use crate::framebuffer;
use alloc::string::ToString;

pub struct SettingsState;

impl SettingsState {
    pub fn render(&self, x: i32, y: i32, w: u32, h: u32) {
        framebuffer::with(|c| {
            c.fill_rect(x, y, w, h, (0x18, 0x18, 0x20));

            let stats = crate::memory::pmm::stats();
            let free_mib = (stats.free_frames * 4096) / (1024 * 1024);
            let total_mib = (stats.total_frames * 4096) / (1024 * 1024);
            let ticks = crate::sched::ticks();

            let lines = [
                "moon OS -- system info".to_string(),
                alloc::format!("memory: {} / {} MiB free", free_mib, total_mib),
                alloc::format!("tasks: {}", crate::sched::task_count()),
                alloc::format!("uptime: {} ticks (~{}s)", ticks, ticks / 100),
            ];
            for (row, line) in lines.iter().enumerate() {
                c.draw_str_at(
                    x + 6,
                    y + 6 + row as i32 * 12,
                    line,
                    (0xD0, 0xD0, 0xD0),
                    None,
                );
            }
        });
    }
}
