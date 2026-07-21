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

pub mod taskbar;
pub mod widgets;
pub mod window;

use crate::framebuffer;
use alloc::string::String;
use alloc::vec::Vec;
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

    let id = gui.next_id;
    gui.next_id += 1;
    gui.windows.push(Window {
        id,
        x: 40,
        y: 30,
        w: 420,
        h: 220,
        title: String::from("Terminal"),
        content: WindowContent::Terminal(TerminalState::new()),
    });

    let id = gui.next_id;
    gui.next_id += 1;
    gui.windows.push(Window {
        id,
        x: 500,
        y: 50,
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
            if let Some(idx) = hit_test(&gui.windows, cx, cy) {
                let w = gui.windows.remove(idx);
                let starts_drag = w.title_bar_contains(cx, cy);
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

    framebuffer::with(|c| {
        c.fill_rect(0, 0, screen_w as u32, screen_h as u32, (0x14, 0x18, 0x22));
    });

    let focused_id = gui.windows.last().map(|w| w.id);
    taskbar::render(&gui.windows, focused_id, screen_w, screen_h);

    for w in gui.windows.iter() {
        w.render(Some(w.id) == focused_id);
    }

    framebuffer::with(|c| {
        c.fill_rect(gui.cursor_x, gui.cursor_y, 4, 4, (0xFF, 0xFF, 0x40));
    });
}
