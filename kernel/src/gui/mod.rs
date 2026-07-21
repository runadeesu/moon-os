//! Window system: a simple compositor over the framebuffer console.
//!
//! There's no per-window offscreen buffer (each widget still draws itself
//! opaquely, not into its own surface), but the whole-screen redraw --
//! desktop background, then each window in z-order, then the cursor -- goes
//! through `Console`'s single shared backbuffer and reaches the display in
//! one `present()` call at the very end, so the screen never shows a
//! half-composited frame. Redraws happen on every input event, plus a
//! periodic tick (see `on_tick`) for ambient animation (drifting clouds,
//! the day/night cycle, open/close flashes) -- a modest, bounded rate
//! chosen to keep a software-only compositor cheap, not a claim of a
//! continuous 60+ fps render loop.
//!
//! Windows host built-in widgets (terminal, settings, ...) rather than
//! separate processes for their *chrome* -- moon OS's usermode (M7/M8) runs
//! real ring-3 processes, but the desktop shell itself is still kernel-side
//! content the window manager draws directly, the same tradeoff most
//! embedded/early-stage window managers make before a real display-server
//! protocol exists.

mod cursor;
mod desktop;
pub mod desktop_widgets;
pub mod dock;
pub mod notifications;
pub mod topbar;
pub mod widgets;
pub mod window;

use crate::framebuffer;
use alloc::string::String;
use alloc::vec::Vec;
use desktop::Star;
use spin::Mutex;
use widgets::{
    files::FileManagerState, settings::SettingsState, store::StoreState, terminal::TerminalState,
};
use window::{TitleButton, Window, WindowContent};

/// Non-character keyboard input the focused window (or the window manager
/// itself, for Alt+Tab) cares about -- arrow keys for history/cursor
/// navigation, Alt+Tab for focus cycling.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SpecialKey {
    Up,
    Down,
    Left,
    Right,
    AltTab,
}

/// A right-click popup menu: `items` are display labels, `actions` the
/// opaque action codes `Window::handle_context_action` interprets --
/// interpretation is up to whichever widget opened the menu.
pub struct ContextMenu {
    pub x: i32,
    pub y: i32,
    pub items: Vec<(String, u32)>,
}

const MENU_ROW_H: i32 = 16;
const MENU_W: i32 = 120;

/// How often (in scheduler ticks, ~100/sec) a redraw happens even with no
/// input, to keep ambient animation (clouds, day/night, clock) moving.
const AMBIENT_REDRAW_TICKS: u64 = 10;

struct GuiState {
    /// z-order: the last entry is topmost and holds keyboard focus.
    windows: Vec<Window>,
    next_id: u32,
    cursor_x: i32,
    cursor_y: i32,
    dragging: Option<usize>,
    resizing: Option<usize>,
    left_was_down: bool,
    right_was_down: bool,
    stars: Vec<Star>,
    context_menu: Option<(usize, ContextMenu)>,
    /// Screen-space snap preview rect shown while dragging near an edge,
    /// applied on release.
    snap_preview: Option<(i32, i32, u32, u32)>,
}

impl GuiState {
    const fn new() -> Self {
        Self {
            windows: Vec::new(),
            next_id: 1,
            cursor_x: 0,
            cursor_y: 0,
            dragging: None,
            resizing: None,
            left_was_down: false,
            right_was_down: false,
            stars: Vec::new(),
            context_menu: None,
            snap_preview: None,
        }
    }
}

static GUI: Mutex<GuiState> = Mutex::new(GuiState::new());

fn screen_size() -> (usize, usize) {
    framebuffer::with(|c| (c.width(), c.height())).unwrap_or((800, 600))
}

/// The desktop's usable content area: clear of the top bar, the left dock,
/// and the right-side widget panel. Used for maximize and edge-snap.
fn content_area(screen_w: usize, screen_h: usize) -> (i32, i32, u32, u32) {
    let x = dock::WIDTH as i32 + 4;
    let y = topbar::HEIGHT as i32 + 4;
    let w = screen_w as u32 - dock::WIDTH - desktop_widgets::PANEL_W - 24;
    let h = screen_h as u32 - topbar::HEIGHT - 8;
    (x, y, w, h)
}

pub fn init() {
    let (screen_w, screen_h) = screen_size();

    let mut gui = GUI.lock();
    gui.cursor_x = (screen_w / 2) as i32;
    gui.cursor_y = (screen_h / 2) as i32;
    gui.stars = desktop::generate_stars(screen_w, screen_h, 150);

    // Positioned clear of the top bar and the left dock; the right side is
    // left open for the calendar/system-monitor desktop widgets.
    let content_top = topbar::HEIGHT as i32 + 16;
    let content_left = dock::WIDTH as i32 + 16;
    let now = crate::sched::ticks();

    let id = gui.next_id;
    gui.next_id += 1;
    gui.windows.push(Window {
        id,
        x: content_left,
        y: content_top,
        w: 460,
        h: 260,
        title: String::from("Terminal"),
        content: WindowContent::Terminal(TerminalState::new()),
        minimized: false,
        maximized: None,
        opened_at: now,
        closing_since: None,
    });

    let id = gui.next_id;
    gui.next_id += 1;
    gui.windows.push(Window {
        id,
        x: content_left,
        y: content_top + 260 + 40,
        w: 300,
        h: 150,
        title: String::from("Settings"),
        content: WindowContent::Settings(SettingsState),
        minimized: false,
        maximized: None,
        opened_at: now,
        closing_since: None,
    });

    let second_col = content_left + 460 + 40;

    let id = gui.next_id;
    gui.next_id += 1;
    gui.windows.push(Window {
        id,
        x: second_col,
        y: content_top,
        w: 420,
        h: 260,
        title: String::from("Files"),
        content: WindowContent::Files(FileManagerState::new()),
        minimized: false,
        maximized: None,
        opened_at: now,
        closing_since: None,
    });

    let id = gui.next_id;
    gui.next_id += 1;
    gui.windows.push(Window {
        id,
        x: second_col,
        y: content_top + 260 + 40,
        w: 420,
        h: 150,
        // Distinct from "Settings" for the dock icon letter (both start
        // with S otherwise); the title bar itself still reads "Moon Store"
        // (see `Window::title_bytes`).
        title: String::from("Moon"),
        content: WindowContent::Store(StoreState::new()),
        minimized: false,
        maximized: None,
        opened_at: now,
        closing_since: None,
    });
    drop(gui);

    redraw();
    crate::serial_println!("gui: initialized, {}x{}", screen_w, screen_h);
}

fn hit_test(windows: &[Window], x: i32, y: i32) -> Option<usize> {
    windows
        .iter()
        .enumerate()
        .rev()
        .find(|(_, w)| w.contains(x, y))
        .map(|(i, _)| i)
}

/// Called from the keyboard IRQ handler with a decoded ASCII byte.
pub fn on_key(ch: u8) {
    {
        let mut gui = GUI.lock();
        if let Some(top) = gui.windows.last_mut() {
            top.handle_char(ch);
        }
    }
    redraw();
}

/// Called from the keyboard IRQ handler for non-character keys (arrows,
/// Alt+Tab).
pub fn on_special_key(key: SpecialKey) {
    {
        let mut gui = GUI.lock();
        if key == SpecialKey::AltTab {
            if gui.windows.len() > 1 {
                if let Some(w) = gui.windows.pop() {
                    gui.windows.insert(0, w);
                }
            }
        } else if let Some(top) = gui.windows.last_mut() {
            top.handle_special_key(key);
        }
    }
    redraw();
}

/// Called from the timer IRQ handler on every tick. Redraws immediately
/// while anything is actively animating (open/close flash, dragging,
/// resizing) so those stay smooth, and otherwise only every
/// `AMBIENT_REDRAW_TICKS` ticks -- enough to keep the clock, clouds, and
/// day/night cycle visibly moving without redrawing the whole desktop at
/// the full timer rate for no visible benefit.
pub fn on_tick(now: u64) {
    let animating = {
        let gui = GUI.lock();
        gui.dragging.is_some()
            || gui.resizing.is_some()
            || gui.windows.iter().any(|w| {
                w.closing_since.is_some()
                    || now.saturating_sub(w.opened_at) < window::OPEN_ANIM_TICKS
            })
    };

    if animating {
        reap_closed_windows(now);
        redraw();
    } else if now.is_multiple_of(AMBIENT_REDRAW_TICKS) {
        redraw();
    }
}

fn reap_closed_windows(now: u64) {
    let mut gui = GUI.lock();
    gui.windows.retain(|w| !w.close_animation_done(now));
}

/// Called from the mouse IRQ handler with a decoded packet. `dy` follows the
/// PS/2 convention (positive = up), so it's subtracted, not added, to reach
/// screen-space (positive = down).
pub fn on_mouse(dx: i32, dy: i32, left: bool, right: bool, _middle: bool) {
    let (screen_w, screen_h) = screen_size();
    let now = crate::sched::ticks();

    {
        let mut gui = GUI.lock();
        gui.cursor_x = (gui.cursor_x + dx).clamp(0, screen_w as i32 - 1);
        gui.cursor_y = (gui.cursor_y - dy).clamp(0, screen_h as i32 - 1);
        let (cx, cy) = (gui.cursor_x, gui.cursor_y);

        // A context menu, if open, captures the next click entirely --
        // either it selects an item, or the click just dismisses it.
        if left && !gui.left_was_down {
            if let Some((owner, menu)) = gui.context_menu.take() {
                let row = (cy - menu.y) / MENU_ROW_H;
                if cx >= menu.x
                    && cx < menu.x + MENU_W
                    && row >= 0
                    && (row as usize) < menu.items.len()
                {
                    let action = menu.items[row as usize].1;
                    if let Some(w) = gui.windows.get_mut(owner) {
                        w.content.handle_context_action(action);
                    }
                }
                gui.left_was_down = left;
                gui.right_was_down = right;
                drop(gui);
                redraw();
                return;
            }
        }

        if right && !gui.right_was_down {
            if let Some(idx) = hit_test(&gui.windows, cx, cy) {
                let w = &gui.windows[idx];
                if !w.title_bar_contains(cx, cy) {
                    let local = (cx - w.x, cy - w.y - window::TITLE_BAR_HEIGHT);
                    let menu = gui.windows[idx].handle_right_click(local.0, local.1);
                    if let Some(mut menu) = menu {
                        menu.x = cx;
                        menu.y = cy;
                        gui.context_menu = Some((idx, menu));
                    }
                }
            }
        }

        if left && !gui.left_was_down {
            gui.context_menu = None;

            if cx < dock::WIDTH as i32 {
                if let Some(idx) = dock::icon_at(gui.windows.len(), topbar::HEIGHT as i32, cx, cy) {
                    if let Some(w) = gui.windows.get_mut(idx) {
                        w.minimized = false;
                    }
                    let w = gui.windows.remove(idx);
                    gui.windows.push(w);
                }
            } else if let Some(idx) = hit_test(&gui.windows, cx, cy) {
                let w = &gui.windows[idx];
                if let Some(btn) = w.button_at(cx, cy) {
                    match btn {
                        TitleButton::Close => {
                            gui.windows[idx].begin_closing(now);
                        }
                        TitleButton::Minimize => {
                            gui.windows[idx].minimized = true;
                        }
                        TitleButton::Maximize => {
                            let (ax, ay, aw, ah) = content_area(screen_w, screen_h);
                            gui.windows[idx].toggle_maximize(ax, ay, aw, ah);
                            let w = gui.windows.remove(idx);
                            gui.windows.push(w);
                        }
                    }
                } else if w.resize_grip_contains(cx, cy) {
                    let w = gui.windows.remove(idx);
                    gui.windows.push(w);
                    gui.resizing = Some(gui.windows.len() - 1);
                } else {
                    let in_title_bar = w.title_bar_contains(cx, cy);
                    let starts_drag = in_title_bar && w.maximized.is_none();

                    let mut w = gui.windows.remove(idx);
                    if !in_title_bar {
                        let (lx, ly) = (cx - w.x, cy - w.y - window::TITLE_BAR_HEIGHT);
                        w.handle_click(lx, ly);
                    }
                    gui.windows.push(w);
                    if starts_drag {
                        gui.dragging = Some(gui.windows.len() - 1);
                    }
                }
            }
        }

        if !left {
            if let Some(idx) = gui.dragging.take() {
                if let Some((sx, sy, sw, sh)) = gui.snap_preview.take() {
                    if let Some(w) = gui.windows.get_mut(idx) {
                        w.x = sx;
                        w.y = sy;
                        w.resize_to(sw, sh - window::TITLE_BAR_HEIGHT as u32);
                    }
                }
            }
            gui.resizing = None;
        } else if let Some(idx) = gui.dragging {
            if let Some(w) = gui.windows.get_mut(idx) {
                w.x += dx;
                w.y -= dy;
            }
            gui.snap_preview = compute_snap_preview(cx, cy, screen_w, screen_h);
        } else if let Some(idx) = gui.resizing {
            if let Some(w) = gui.windows.get_mut(idx) {
                let new_w = (w.w as i32 + dx).max(window::MIN_W as i32) as u32;
                let new_h = (w.h as i32 - dy).max(window::MIN_H as i32) as u32;
                w.resize_to(new_w, new_h);
            }
        }

        gui.left_was_down = left;
        gui.right_was_down = right;
    }

    redraw();
}

/// While dragging, returns the snap-target rect the window would jump to
/// if released now: left/right screen edges snap to a half-width column,
/// the top edge (top bar) snaps to maximize.
fn compute_snap_preview(
    cx: i32,
    cy: i32,
    screen_w: usize,
    screen_h: usize,
) -> Option<(i32, i32, u32, u32)> {
    let (ax, ay, aw, ah) = content_area(screen_w, screen_h);
    const EDGE: i32 = 6;
    if cy <= topbar::HEIGHT as i32 + EDGE {
        Some((ax, ay, aw, ah))
    } else if cx <= ax + EDGE {
        Some((ax, ay, aw / 2, ah))
    } else if cx >= ax + aw as i32 - EDGE {
        Some((ax + aw as i32 / 2, ay, aw / 2, ah))
    } else {
        None
    }
}

pub fn redraw() {
    let gui = GUI.lock();
    let (screen_w, screen_h) = screen_size();
    let now = crate::sched::ticks();

    desktop::render(&gui.stars, screen_w, screen_h, now);
    desktop_widgets::render(
        screen_w as i32 - desktop_widgets::PANEL_W as i32 - 16,
        topbar::HEIGHT as i32 + 16,
    );

    let focused_id = gui
        .windows
        .iter()
        .rev()
        .find(|w| !w.minimized)
        .map(|w| w.id);
    for w in gui.windows.iter() {
        if !w.minimized {
            w.render(Some(w.id) == focused_id, now);
        }
    }

    if let Some((sx, sy, sw, sh)) = gui.snap_preview {
        framebuffer::with(|c| {
            c.blend_rect(sx, sy, sw, sh, (0x30, 0xE0, 0xFF), 60);
        });
    }

    // The top bar and dock are OS chrome, always drawn above every app
    // window -- same convention as a real desktop's menu bar/dock.
    topbar::render(screen_w);
    dock::render(&gui.windows, focused_id, screen_h, topbar::HEIGHT as i32);
    notifications::render(screen_w as i32 - 16, topbar::HEIGHT as i32 + 12);

    if let Some((_, menu)) = &gui.context_menu {
        render_context_menu(menu);
    }

    framebuffer::with(|c| {
        let hovering_clickable = gui.dragging.is_none()
            && (gui.cursor_x < dock::WIDTH as i32
                || gui.windows.iter().any(|w| {
                    !w.minimized
                        && (w.button_at(gui.cursor_x, gui.cursor_y).is_some()
                            || w.resize_grip_contains(gui.cursor_x, gui.cursor_y))
                }));
        cursor::draw(c, gui.cursor_x, gui.cursor_y, hovering_clickable);
        // Everything above this point drew into the offscreen backbuffer;
        // this is the one point where the frame actually reaches the
        // display, so the screen never shows a half-composited redraw.
        c.present();
    });
}

fn render_context_menu(menu: &ContextMenu) {
    let h = menu.items.len() as u32 * MENU_ROW_H as u32;
    framebuffer::with(|c| {
        c.glow_border(menu.x, menu.y, MENU_W as u32, h, (0x30, 0xE0, 0xFF));
        c.blend_rect(menu.x, menu.y, MENU_W as u32, h, (0x14, 0x18, 0x24), 235);
        for (i, (label, _)) in menu.items.iter().enumerate() {
            let row_y = menu.y + i as i32 * MENU_ROW_H;
            c.draw_str_at(menu.x + 6, row_y + 4, label, (0xE0, 0xE0, 0xE0), None);
        }
    });
}
