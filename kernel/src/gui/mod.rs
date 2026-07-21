//! Window system: a simple compositor over the framebuffer console.
//!
//! There's no per-window offscreen buffer or double buffering -- every
//! redraw repaints the desktop background, then each window in z-order,
//! then the cursor, directly into the real framebuffer. That's flicker-prone
//! compared to a real compositor, but there's no vsync to pace against yet
//! either, and redraws only happen on input (not every frame), so it's a
//! reasonable simplification for now.
//!
//! Windows host built-in widgets (terminal, settings) rather than separate
//! processes: moon OS has no usermode or ELF loader yet (that's M7/M8), so
//! "applications" for now are kernel-side content the window manager draws.

mod desktop;
pub mod desktop_widgets;
pub mod dock;
pub mod topbar;
pub mod widgets;
pub mod window;

use crate::framebuffer;
use alloc::string::String;
use alloc::vec::Vec;
use desktop::Star;
use spin::Mutex;
use widgets::{settings::SettingsState, terminal::TerminalState};
use window::{Window, WindowContent};

struct GuiState {
    /// z-order: the last entry is topmost and holds keyboard focus.
    windows: Vec<Window>,
    next_id: u32,
    cursor_x: i32,
    cursor_y: i32,
    dragging: Option<usize>,
    left_was_down: bool,
    stars: Vec<Star>,
}

impl GuiState {
    const fn new() -> Self {
        Self {
            windows: Vec::new(),
            next_id: 1,
            cursor_x: 0,
            cursor_y: 0,
            dragging: None,
            left_was_down: false,
            stars: Vec::new(),
        }
    }
}

static GUI: Mutex<GuiState> = Mutex::new(GuiState::new());

fn screen_size() -> (usize, usize) {
    framebuffer::with(|c| (c.width(), c.height())).unwrap_or((800, 600))
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

/// Called from the mouse IRQ handler with a decoded packet. `dy` follows the
/// PS/2 convention (positive = up), so it's subtracted, not added, to reach
/// screen-space (positive = down).
pub fn on_mouse(dx: i32, dy: i32, left: bool, _right: bool, _middle: bool) {
    let (screen_w, screen_h) = screen_size();

    {
        let mut gui = GUI.lock();
        gui.cursor_x = (gui.cursor_x + dx).clamp(0, screen_w as i32 - 1);
        gui.cursor_y = (gui.cursor_y - dy).clamp(0, screen_h as i32 - 1);

        if left && !gui.left_was_down {
            let (cx, cy) = (gui.cursor_x, gui.cursor_y);
            if cx < dock::WIDTH as i32 {
                if let Some(idx) = dock::icon_at(gui.windows.len(), topbar::HEIGHT as i32, cx, cy) {
                    let w = gui.windows.remove(idx);
                    gui.windows.push(w);
                }
            } else if let Some(idx) = hit_test(&gui.windows, cx, cy) {
                let w = &gui.windows[idx];
                let starts_drag = w.title_bar_contains(cx, cy);
                let content_click =
                    (!starts_drag).then_some((cx - w.x, cy - w.y - window::TITLE_BAR_HEIGHT));

                let mut w = gui.windows.remove(idx);
                if let Some((lx, ly)) = content_click {
                    w.handle_click(lx, ly);
                }
                gui.windows.push(w);
                if starts_drag {
                    gui.dragging = Some(gui.windows.len() - 1);
                }
            }
        }

        if !left {
            gui.dragging = None;
        } else if let Some(idx) = gui.dragging {
            if let Some(w) = gui.windows.get_mut(idx) {
                w.x += dx;
                w.y -= dy;
            }
        }

        gui.left_was_down = left;
    }

    redraw();
}

pub fn redraw() {
    let gui = GUI.lock();
    let (screen_w, screen_h) = screen_size();

    desktop::render(&gui.stars, screen_w, screen_h);
    desktop_widgets::render(
        screen_w as i32 - desktop_widgets::PANEL_W as i32 - 16,
        topbar::HEIGHT as i32 + 16,
    );

    let focused_id = gui.windows.last().map(|w| w.id);
    for w in gui.windows.iter() {
        w.render(Some(w.id) == focused_id);
    }

    // The top bar and dock are OS chrome, always drawn above every app
    // window -- same convention as a real desktop's menu bar/dock.
    topbar::render(screen_w);
    dock::render(&gui.windows, focused_id, screen_h, topbar::HEIGHT as i32);

    framebuffer::with(|c| {
        c.fill_rect(gui.cursor_x, gui.cursor_y, 4, 4, (0xFF, 0xFF, 0x40));
    });
}
