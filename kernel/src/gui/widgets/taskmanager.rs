//! A read-only Task Manager: the scheduler's real, current task list
//! (`sched::list_tasks`) -- id and kernel/user-mode, live every redraw.
//! No CPU% column, because there's no per-task CPU-time accounting in the
//! scheduler to report (adding one honestly is future work); no "End Task"
//! button either, since the scheduler has no safe way to tear down an
//! arbitrary task from the outside yet (only a task can exit itself, via
//! `sched::exit_current`) -- a button that can't actually kill anything
//! would be exactly the kind of dead control this project avoids.

use crate::framebuffer;
use alloc::format;

const ROW_H: i32 = 16;
const HEADER_H: i32 = 20;

pub struct TaskManagerState;

impl TaskManagerState {
    pub fn render(&self, x: i32, y: i32, w: u32, h: u32) {
        let tasks = crate::sched::list_tasks();

        framebuffer::with(|c| {
            c.fill_rect(x, y, w, h, (0x10, 0x10, 0x16));
            c.draw_str_at(
                x + 4,
                y + 4,
                &format!("Task Manager -- {} running", tasks.len()),
                (0x90, 0xC0, 0xFF),
                None,
            );

            for (row, task) in tasks.iter().enumerate() {
                let row_y = y + HEADER_H + row as i32 * ROW_H;
                if row_y + ROW_H > y + h as i32 {
                    break;
                }
                let kind = if task.is_user { "user" } else { "kernel" };
                c.draw_str_at(
                    x + 8,
                    row_y,
                    &format!("#{:<4} {}", task.id, kind),
                    (0xD0, 0xD0, 0xD0),
                    None,
                );
            }
        });
    }
}

impl Default for TaskManagerState {
    fn default() -> Self {
        Self
    }
}
