//! A window: a titled, draggable, focusable rectangle on the desktop,
//! hosting one built-in widget.

use super::widgets::{
    files::FileManagerState, settings::SettingsState, store::StoreState, terminal::TerminalState,
};
use crate::framebuffer;
use alloc::string::String;

pub const TITLE_BAR_HEIGHT: i32 = 18;

pub enum WindowContent {
    Terminal(TerminalState),
    Settings(SettingsState),
    Files(FileManagerState),
    Store(StoreState),
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

    /// `x`/`y` are local to the content area (below the title bar) --
    /// forwarded here from `gui::on_mouse` for clicks that land inside a
    /// window's body rather than its title bar.
    pub fn handle_click(&mut self, x: i32, y: i32) {
        match &mut self.content {
            WindowContent::Settings(settings) => settings.handle_click(x, y),
            WindowContent::Files(files) => files.handle_click(x, y),
            WindowContent::Store(store) => store.handle_click(x, y),
            WindowContent::Terminal(_) => {}
        }
    }

    /// The title text: fixed English for content that's a technical
    /// term/proper noun (Terminal, File Manager, Moon Store -- like a real
    /// desktop keeps app/protocol names untranslated), and
    /// `crate::i18n`-translated for Settings.
    fn title_bytes(&self) -> &'static [u8] {
        match &self.content {
            WindowContent::Terminal(_) => b"Terminal",
            WindowContent::Settings(_) => crate::i18n::tr(crate::i18n::Key::SettingsTitle),
            WindowContent::Files(_) => b"File Manager",
            WindowContent::Store(_) => b"Moon Store",
        }
    }

    pub fn render(&self, focused: bool) {
        const NEON: (u8, u8, u8) = (0x30, 0xE0, 0xFF);
        let border = if focused { NEON } else { (0x50, 0x50, 0x58) };
        let title_bg = if focused {
            (0x0C, 0x28, 0x38)
        } else {
            (0x30, 0x30, 0x38)
        };
        let outer_h = self.total_height() as u32 + 2;

        framebuffer::with(|c| {
            if focused {
                c.glow_border(self.x - 1, self.y - 1, self.w + 2, outer_h, NEON);
            }
            c.fill_rect(self.x - 1, self.y - 1, self.w + 2, outer_h, border);
            c.fill_rect(self.x, self.y, self.w, TITLE_BAR_HEIGHT as u32, title_bg);
            c.draw_glyphs_at(
                self.x + 4,
                self.y + 5,
                self.title_bytes(),
                (0xFF, 0xFF, 0xFF),
                None,
            );
            if focused {
                c.draw_corner_brackets(self.x - 1, self.y - 1, self.w + 2, outer_h, NEON);
            }
        });

        let content_y = self.y + TITLE_BAR_HEIGHT;
        match &self.content {
            WindowContent::Terminal(terminal) => terminal.render(self.x, content_y, self.w, self.h),
            WindowContent::Settings(settings) => settings.render(self.x, content_y, self.w, self.h),
            WindowContent::Files(files) => files.render(self.x, content_y, self.w, self.h),
            WindowContent::Store(store) => store.render(self.x, content_y, self.w, self.h),
        }
    }
}
