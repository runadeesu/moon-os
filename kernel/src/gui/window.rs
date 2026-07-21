//! A window: a titled, draggable, focusable rectangle on the desktop,
//! hosting one built-in widget.

use super::widgets::{settings::SettingsState, terminal::TerminalState};
use crate::framebuffer;
use alloc::string::String;

pub const TITLE_BAR_HEIGHT: i32 = 18;

pub enum WindowContent {
    Terminal(TerminalState),
    Settings(SettingsState),
}

pub struct Window {
    pub id: u32,
    pub x: i32,
    pub y: i32,
    /// Content area size -- the title bar sits above this, adding
    /// `TITLE_BAR_HEIGHT` to the window's total on-screen footprint.
    pub w: u32,
    pub h: u32,
    pub title: String,
    pub content: WindowContent,
}

impl Window {
    pub fn total_height(&self) -> i32 {
        TITLE_BAR_HEIGHT + self.h as i32
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && px < self.x + self.w as i32
            && py >= self.y
            && py < self.y + self.total_height()
    }

    pub fn title_bar_contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && px < self.x + self.w as i32
            && py >= self.y
            && py < self.y + TITLE_BAR_HEIGHT
    }

    pub fn handle_char(&mut self, ch: u8) {
        if let WindowContent::Terminal(terminal) = &mut self.content {
            terminal.handle_char(ch);
        }
    }

    pub fn render(&self, focused: bool) {
        let border = if focused {
            (0x50, 0x90, 0xE0)
        } else {
            (0x50, 0x50, 0x58)
        };
        let title_bg = if focused {
            (0x1E, 0x50, 0x90)
        } else {
            (0x30, 0x30, 0x38)
        };

        framebuffer::with(|c| {
            c.fill_rect(
                self.x - 1,
                self.y - 1,
                self.w + 2,
                self.total_height() as u32 + 2,
                border,
            );
            c.fill_rect(self.x, self.y, self.w, TITLE_BAR_HEIGHT as u32, title_bg);
            c.draw_str_at(
                self.x + 4,
                self.y + 5,
                &self.title,
                (0xFF, 0xFF, 0xFF),
                None,
            );
        });

        let content_y = self.y + TITLE_BAR_HEIGHT;
        match &self.content {
            WindowContent::Terminal(terminal) => terminal.render(self.x, content_y, self.w, self.h),
            WindowContent::Settings(settings) => settings.render(self.x, content_y, self.w, self.h),
        }
    }
}
