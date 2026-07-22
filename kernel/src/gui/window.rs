//! A window: a titled, draggable, resizable, focusable rectangle on the
//! desktop, hosting one built-in widget, with real minimize/maximize/close
//! buttons and short open/close flash animations.
//!
//! There's no per-pixel alpha compositing of widget content (each widget
//! draws itself opaquely), so "animation" here is geometry- and
//! overlay-based rather than true alpha-blended content: open/close are a
//! brief white/background flash faded via `blend_rect`, not a smooth
//! resize or cross-fade of the actual content pixels. That's an honest
//! simplification, not a placeholder -- doing better would mean giving
//! every widget an offscreen buffer to composite, a bigger structural
//! change than this pass makes.

use super::widgets::{
    calculator::CalculatorState, files::FileManagerState, moon_ai::MoonAiState, notes::NotesState,
    settings::SettingsState, store::StoreState, taskmanager::TaskManagerState,
    terminal::TerminalState,
};
use crate::framebuffer;
use alloc::string::String;

pub const TITLE_BAR_HEIGHT: i32 = 18;

const BTN_SIZE: i32 = 12;
const BTN_GAP: i32 = 4;
const RESIZE_GRIP: i32 = 10;
pub const MIN_W: u32 = 180;
pub const MIN_H: u32 = 100;

pub const OPEN_ANIM_TICKS: u64 = 8;
pub const CLOSE_ANIM_TICKS: u64 = 10;
pub const RESIZE_ANIM_TICKS: u64 = 10;
pub const MINIMIZE_ANIM_TICKS: u64 = 12;

/// Which geometry transition a window is currently mid-way through, eased
/// over `duration` ticks from `from` to `to` -- `Window::display_rect` is
/// the only thing that reads the interpolation itself, so both chrome and
/// (since widgets already re-layout correctly at arbitrary sizes, proven by
/// live drag-resize) content genuinely animate at each intermediate size
/// rather than just fading an overlay on top of the final size. `kind` is
/// read by `gui` to know when a finished `Minimize` animation should
/// actually flip `minimized`, since that window keeps rendering (shrinking
/// toward its taskbar icon) for a few ticks after the click.
#[derive(PartialEq, Eq)]
pub enum AnimKind {
    Open,
    Close,
    Minimize,
    Unminimize,
    /// Maximize/restore-from-maximize: `maximized` is already toggled the
    /// instant the button is clicked, this only smooths out the geometry
    /// the eye sees getting there.
    Resize,
}

pub struct WindowAnim {
    pub kind: AnimKind,
    start: u64,
    duration: u64,
    from: (i32, i32, u32, u32),
    to: (i32, i32, u32, u32),
}

impl WindowAnim {
    pub fn new(
        kind: AnimKind,
        start: u64,
        duration: u64,
        from: (i32, i32, u32, u32),
        to: (i32, i32, u32, u32),
    ) -> Self {
        Self {
            kind,
            start,
            duration,
            from,
            to,
        }
    }

    fn progress_permille(&self, now: u64) -> u32 {
        let elapsed = now.saturating_sub(self.start).min(self.duration);
        let p = (elapsed * 1000 / self.duration.max(1)) as u32;
        // Quadratic ease-out: fast start, gentle settle -- reads as a real
        // spring-ish window animation rather than a linear slide.
        let inv = 1000 - p;
        1000 - inv * inv / 1000
    }

    pub fn done(&self, now: u64) -> bool {
        now.saturating_sub(self.start) >= self.duration
    }
}

fn lerp_i32(a: i32, b: i32, p: u32) -> i32 {
    a + (b - a) * p as i32 / 1000
}

fn lerp_u32(a: u32, b: u32, p: u32) -> u32 {
    (a as i32 + (b as i32 - a as i32) * p as i32 / 1000).max(1) as u32
}

pub enum WindowContent {
    Terminal(TerminalState),
    Settings(SettingsState),
    Files(FileManagerState),
    Store(StoreState),
    Notes(NotesState),
    Calculator(CalculatorState),
    TaskManager(TaskManagerState),
    MoonAi(MoonAiState),
}

impl WindowContent {
    /// Dispatches a context-menu action code (see `handle_right_click`) to
    /// whichever widget opened the menu.
    pub fn handle_context_action(&mut self, action: u32) {
        if let WindowContent::Files(files) = self {
            files.handle_context_action(action);
        }
    }
}

/// Which title-bar button (if any) a point lands on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TitleButton {
    Minimize,
    Maximize,
    Close,
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
    pub minimized: bool,
    /// Geometry to restore on un-maximize; `Some` iff currently maximized.
    pub maximized: Option<(i32, i32, u32, u32)>,
    /// Tick this window was created on -- drives the brief open flash.
    pub opened_at: u64,
    /// Tick the close button was clicked on, if a close is in progress --
    /// the window keeps rendering (fading out) until `CLOSE_ANIM_TICKS`
    /// later, when `gui` actually drops it.
    pub closing_since: Option<u64>,
    /// A geometry transition in progress (open/close/maximize/restore) --
    /// see `WindowAnim` and `display_rect`.
    pub anim: Option<WindowAnim>,
}

impl Window {
    pub fn total_height(&self) -> i32 {
        TITLE_BAR_HEIGHT + self.h as i32
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        !self.minimized
            && px >= self.x
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

    /// Button positions for an arbitrary title-bar rect -- shared by
    /// `button_at` (always the real, logical geometry, so clicks hit-test
    /// correctly even mid-animation) and `render` (the animated display
    /// rect, so the buttons never visually detach from the frame around
    /// them the way an early version of this animation code did).
    fn button_rects_at(x: i32, y: i32, w: u32) -> [(TitleButton, i32, i32); 3] {
        let top = y + (TITLE_BAR_HEIGHT - BTN_SIZE) / 2;
        let mut bx = x + w as i32 - BTN_SIZE - 4;
        let close = (TitleButton::Close, bx, top);
        bx -= BTN_SIZE + BTN_GAP;
        let maximize = (TitleButton::Maximize, bx, top);
        bx -= BTN_SIZE + BTN_GAP;
        let minimize = (TitleButton::Minimize, bx, top);
        [minimize, maximize, close]
    }

    pub fn button_at(&self, px: i32, py: i32) -> Option<TitleButton> {
        if !self.title_bar_contains(px, py) {
            return None;
        }
        Self::button_rects_at(self.x, self.y, self.w)
            .into_iter()
            .find(|&(_, bx, by)| px >= bx && px < bx + BTN_SIZE && py >= by && py < by + BTN_SIZE)
            .map(|(btn, _, _)| btn)
    }

    /// True if `(px, py)` lands on the bottom-right resize grip -- only
    /// while not maximized (a maximized window fills its slot, nothing to
    /// resize).
    pub fn resize_grip_contains(&self, px: i32, py: i32) -> bool {
        self.maximized.is_none()
            && px >= self.x + self.w as i32 - RESIZE_GRIP
            && px < self.x + self.w as i32
            && py >= self.y + self.total_height() - RESIZE_GRIP
            && py < self.y + self.total_height()
    }

    pub fn resize_to(&mut self, new_w: u32, new_h: u32) {
        self.w = new_w.max(MIN_W);
        self.h = new_h.max(MIN_H);
    }

    /// The rect actually drawn this frame: the logical `(x, y, w, h)`
    /// normally, or an eased in-between rect while `anim` is playing.
    /// Widgets already re-layout correctly at whatever size they're given
    /// (proven by live drag-resize), so animating this genuinely animates
    /// content, not just an overlay on top of the final size.
    fn display_rect(&self, now: u64) -> (i32, i32, u32, u32) {
        match &self.anim {
            Some(anim) if !anim.done(now) => {
                let p = anim.progress_permille(now);
                let (fx, fy, fw, fh) = anim.from;
                let (tx, ty, tw, th) = anim.to;
                (
                    lerp_i32(fx, tx, p),
                    lerp_i32(fy, ty, p),
                    lerp_u32(fw, tw, p),
                    lerp_u32(fh, th, p),
                )
            }
            _ => (self.x, self.y, self.w, self.h),
        }
    }

    /// Toggles maximized state, filling `(area_x, area_y, area_w, area_h)`
    /// (the desktop's usable content area, clear of the top bar/dock/side
    /// panel) when maximizing, or restoring the pre-maximize geometry when
    /// un-maximizing.
    pub fn toggle_maximize(&mut self, area_x: i32, area_y: i32, area_w: u32, area_h: u32) {
        if let Some((x, y, w, h)) = self.maximized.take() {
            self.x = x;
            self.y = y;
            self.w = w;
            self.h = h;
        } else {
            self.maximized = Some((self.x, self.y, self.w, self.h));
            self.x = area_x;
            self.y = area_y;
            self.w = area_w;
            self.h = area_h.saturating_sub(TITLE_BAR_HEIGHT as u32);
        }
    }

    pub fn begin_closing(&mut self, now: u64) {
        if self.closing_since.is_none() {
            self.closing_since = Some(now);
        }
    }

    /// Whether `now` is past this window's close animation -- `gui` uses
    /// this to know when to actually drop it from the window list.
    pub fn close_animation_done(&self, now: u64) -> bool {
        self.closing_since
            .is_some_and(|since| now.saturating_sub(since) >= CLOSE_ANIM_TICKS)
    }

    pub fn handle_char(&mut self, ch: u8) {
        match &mut self.content {
            WindowContent::Terminal(terminal) => terminal.handle_char(ch),
            // Both widgets' search boxes need typed input too -- previously
            // unwired, so search wasn't a real feature, just a display.
            WindowContent::Files(files) => files.handle_char(ch),
            WindowContent::Store(store) => store.handle_char(ch),
            WindowContent::Notes(notes) => notes.handle_char(ch),
            WindowContent::MoonAi(ai) => ai.handle_char(ch),
            WindowContent::Settings(_)
            | WindowContent::Calculator(_)
            | WindowContent::TaskManager(_) => {}
        }
    }

    pub fn handle_special_key(&mut self, key: super::SpecialKey) {
        if let WindowContent::Terminal(terminal) = &mut self.content {
            terminal.handle_special_key(key);
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
            WindowContent::Calculator(calc) => calc.handle_click(x, y),
            WindowContent::Terminal(_)
            | WindowContent::Notes(_)
            | WindowContent::TaskManager(_)
            | WindowContent::MoonAi(_) => {}
        }
    }

    /// `x`/`y` are local to the content area, same convention as
    /// `handle_click`; forwarded for right-clicks (context menus).
    pub fn handle_right_click(&mut self, x: i32, y: i32) -> Option<super::ContextMenu> {
        match &mut self.content {
            WindowContent::Files(files) => files.handle_right_click(x, y),
            _ => None,
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
            WindowContent::Notes(_) => b"Notes",
            WindowContent::Calculator(_) => b"Calculator",
            WindowContent::TaskManager(_) => b"Task Manager",
            WindowContent::MoonAi(_) => b"Moon AI",
        }
    }

    pub fn render(&self, focused: bool, now: u64) {
        let neon = super::theme::accent();
        let border = if focused { neon } else { (0x50, 0x50, 0x58) };
        let title_bg = if focused {
            (0x0C, 0x28, 0x38)
        } else {
            (0x30, 0x30, 0x38)
        };

        let (dx, dy, dw, dh) = self.display_rect(now);
        let outer_h = TITLE_BAR_HEIGHT as u32 + dh + 2;

        // A soft drop shadow: a handful of progressively larger, fainter
        // offset rects behind the window -- cheap compared to a real blur,
        // but reads as "floating above the desktop" from a normal viewing
        // distance.
        framebuffer::with(|c| {
            for (offset, alpha) in [(3, 70u8), (6, 40), (9, 20)] {
                c.blend_rect(
                    dx - 1 + offset,
                    dy - 1 + offset,
                    dw + 2,
                    outer_h,
                    (0x00, 0x00, 0x00),
                    alpha,
                );
            }
        });

        framebuffer::with(|c| {
            if focused {
                c.glow_border(dx - 1, dy - 1, dw + 2, outer_h, neon);
            }
            c.fill_rect(dx - 1, dy - 1, dw + 2, outer_h, border);
            c.fill_rect(dx, dy, dw, TITLE_BAR_HEIGHT as u32, title_bg);
            c.draw_glyphs_at(dx + 4, dy + 5, self.title_bytes(), (0xFF, 0xFF, 0xFF), None);

            for (btn, bx, by) in Self::button_rects_at(dx, dy, dw) {
                let color = match btn {
                    TitleButton::Close => (0xE0, 0x50, 0x50),
                    TitleButton::Maximize => (0x50, 0xC0, 0xE0),
                    TitleButton::Minimize => (0xC0, 0xC0, 0x50),
                };
                c.fill_rect(bx, by, BTN_SIZE as u32, BTN_SIZE as u32, color);
                let glyph = match btn {
                    TitleButton::Close => b'X',
                    TitleButton::Maximize => {
                        if self.maximized.is_some() {
                            b'='
                        } else {
                            b'#'
                        }
                    }
                    TitleButton::Minimize => b'_',
                };
                c.draw_char_at(bx + 2, by, glyph, (0x10, 0x10, 0x14), None);
            }

            if self.maximized.is_none() {
                c.fill_rect(
                    dx + dw as i32 - RESIZE_GRIP,
                    dy + TITLE_BAR_HEIGHT + dh as i32 - RESIZE_GRIP,
                    RESIZE_GRIP as u32,
                    RESIZE_GRIP as u32,
                    border,
                );
            }

            if focused {
                c.draw_corner_brackets(dx - 1, dy - 1, dw + 2, outer_h, neon);
            }
        });

        let content_y = dy + TITLE_BAR_HEIGHT;
        match &self.content {
            WindowContent::Terminal(terminal) => terminal.render(dx, content_y, dw, dh),
            WindowContent::Settings(settings) => settings.render(dx, content_y, dw, dh),
            WindowContent::Files(files) => files.render(dx, content_y, dw, dh),
            WindowContent::Store(store) => store.render(dx, content_y, dw, dh),
            WindowContent::Notes(notes) => notes.render(dx, content_y, dw, dh),
            WindowContent::Calculator(calc) => calc.render(dx, content_y, dw, dh),
            WindowContent::TaskManager(tm) => tm.render(dx, content_y, dw, dh),
            WindowContent::MoonAi(ai) => ai.render(dx, content_y, dw, dh),
        }

        // A brief fade-in/fade-out overlay layered on top of the geometry
        // animation above -- opening/closing windows both grow/shrink *and*
        // fade, closing feels distinct from just "shrinking".
        let flash_alpha = if let Some(since) = self.closing_since {
            let elapsed = now.saturating_sub(since).min(CLOSE_ANIM_TICKS);
            (180 * elapsed / CLOSE_ANIM_TICKS) as u8
        } else {
            let elapsed = now.saturating_sub(self.opened_at).min(OPEN_ANIM_TICKS);
            (180 - 180 * elapsed / OPEN_ANIM_TICKS) as u8
        };
        if flash_alpha > 0 {
            framebuffer::with(|c| {
                c.blend_rect(
                    dx,
                    dy,
                    dw,
                    TITLE_BAR_HEIGHT as u32 + dh,
                    (0xF0, 0xF8, 0xFF),
                    flash_alpha,
                );
            });
        }
    }
}
