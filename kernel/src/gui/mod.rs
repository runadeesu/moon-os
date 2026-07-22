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
pub mod desktop_icons;
pub mod desktop_widgets;
pub mod login;
pub mod mouse_speed;
pub mod notifications;
pub mod taskbar;
pub mod theme;
pub mod widgets;
pub mod window;

use crate::framebuffer;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use desktop::Star;
use spin::Mutex;
use widgets::{
    calculator::CalculatorState, files::FileManagerState, moon_ai::MoonAiState, notes::NotesState,
    settings::SettingsState, store::StoreState, taskmanager::TaskManagerState,
    terminal::TerminalState,
};
use window::{TitleButton, Window, WindowContent};

/// Every app the Moon-button launcher can open. `Store`/`Files`/`Terminal`/
/// `Settings` also exist as the windows already open at boot; picking one
/// from the menu when it's already open just focuses it instead of spawning
/// a second copy, same as clicking a running app's dock icon would.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AppKind {
    Terminal,
    Settings,
    Files,
    Store,
    Notes,
    Calculator,
    TaskManager,
    MoonAi,
}

impl AppKind {
    const ALL: [AppKind; 8] = [
        AppKind::Terminal,
        AppKind::Settings,
        AppKind::Files,
        AppKind::Store,
        AppKind::Notes,
        AppKind::Calculator,
        AppKind::TaskManager,
        AppKind::MoonAi,
    ];

    fn label(self) -> &'static str {
        match self {
            AppKind::Terminal => "Terminal",
            AppKind::Settings => "Settings",
            AppKind::Files => "Files",
            AppKind::Store => "Moon Store",
            AppKind::Notes => "Notes",
            AppKind::Calculator => "Calculator",
            AppKind::TaskManager => "Task Manager",
            AppKind::MoonAi => "Moon AI",
        }
    }

    /// Matches a window's dock-letter `title` field back to the app kind it
    /// was spawned as, so the launcher can focus an already-open window
    /// instead of piling up duplicates.
    fn dock_title(self) -> &'static str {
        match self {
            AppKind::Terminal => "Terminal",
            AppKind::Settings => "Settings",
            AppKind::Files => "Files",
            AppKind::Store => "Moon",
            AppKind::Notes => "Notes",
            AppKind::Calculator => "Calc",
            AppKind::TaskManager => "Jobs",
            AppKind::MoonAi => "AI",
        }
    }

    fn new_content(self) -> WindowContent {
        match self {
            AppKind::Terminal => WindowContent::Terminal(TerminalState::new()),
            AppKind::Settings => WindowContent::Settings(SettingsState),
            AppKind::Files => WindowContent::Files(FileManagerState::new()),
            AppKind::Store => WindowContent::Store(StoreState::new()),
            AppKind::Notes => WindowContent::Notes(NotesState::new()),
            AppKind::Calculator => WindowContent::Calculator(CalculatorState::new()),
            AppKind::TaskManager => WindowContent::TaskManager(TaskManagerState),
            AppKind::MoonAi => WindowContent::MoonAi(MoonAiState::new()),
        }
    }

    fn default_size(self) -> (u32, u32) {
        match self {
            AppKind::Terminal => (460, 260),
            AppKind::Settings => (300, 174),
            AppKind::Files => (420, 260),
            AppKind::Store => (420, 150),
            AppKind::Notes => (360, 260),
            AppKind::Calculator => (200, 260),
            AppKind::TaskManager => (280, 220),
            AppKind::MoonAi => (380, 240),
        }
    }

    /// Maps the short keys `widgets::moon_ai`'s command parser uses
    /// (`crate::gui::request_open("settings")` etc.) back to a kind.
    fn from_key(key: &str) -> Option<AppKind> {
        Some(match key {
            "terminal" => AppKind::Terminal,
            "settings" => AppKind::Settings,
            "files" => AppKind::Files,
            "store" => AppKind::Store,
            "notes" => AppKind::Notes,
            "calculator" => AppKind::Calculator,
            "taskmanager" => AppKind::TaskManager,
            "moonai" => AppKind::MoonAi,
            _ => return None,
        })
    }
}

/// Apps that always show a taskbar icon, running or not -- the "quick
/// launch" row a real desktop's taskbar keeps pinned. Anything else (Notes,
/// Calculator, Task Manager) only gets an icon while it's actually open, via
/// `taskbar_app_targets`.
const PINNED_APPS: [AppKind; 5] = [
    AppKind::Terminal,
    AppKind::Files,
    AppKind::Store,
    AppKind::Settings,
    AppKind::MoonAi,
];

/// One slot in the taskbar's center icon row: either a pinned app (which may
/// or may not currently have a window) or an already-open window that isn't
/// pinned. `usize` indexes into the `windows` slice passed alongside.
#[derive(Clone, Copy)]
enum TaskbarSlot {
    Pinned(AppKind),
    Window(usize),
}

fn taskbar_app_targets(windows: &[Window]) -> Vec<TaskbarSlot> {
    let mut slots: Vec<TaskbarSlot> = PINNED_APPS
        .iter()
        .map(|k| TaskbarSlot::Pinned(*k))
        .collect();
    for (i, w) in windows.iter().enumerate() {
        if !PINNED_APPS.iter().any(|k| k.dock_title() == w.title) {
            slots.push(TaskbarSlot::Window(i));
        }
    }
    slots
}

fn taskbar_icon_view(
    slot: &TaskbarSlot,
    windows: &[Window],
    focused_id: Option<u32>,
) -> taskbar::AppIcon {
    match slot {
        TaskbarSlot::Pinned(kind) => {
            let letter = kind
                .dock_title()
                .as_bytes()
                .first()
                .copied()
                .unwrap_or(b'?') as char;
            match windows.iter().find(|w| w.title == kind.dock_title()) {
                Some(w) => taskbar::AppIcon {
                    letter,
                    running: true,
                    focused: Some(w.id) == focused_id,
                },
                None => taskbar::AppIcon {
                    letter,
                    running: false,
                    focused: false,
                },
            }
        }
        TaskbarSlot::Window(i) => {
            let w = &windows[*i];
            taskbar::AppIcon {
                letter: w.title.as_bytes().first().copied().unwrap_or(b'?') as char,
                running: true,
                focused: Some(w.id) == focused_id,
            }
        }
    }
}

/// A result the taskbar search box found, real either way: an app it can
/// open, or a RAMFS path it can jump the File Manager to.
enum SearchHit {
    App(AppKind),
    File(String),
}

fn run_search(query: &str) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    if query.is_empty() {
        return hits;
    }
    let needle = query.to_ascii_lowercase();
    for kind in AppKind::ALL {
        if kind.label().to_ascii_lowercase().contains(&needle) {
            hits.push(SearchHit::App(kind));
        }
    }
    let root = crate::fs::root().lock();
    for path in root.list() {
        if path.to_ascii_lowercase().contains(&needle) {
            hits.push(SearchHit::File(String::from(path)));
            if hits.len() >= 8 {
                break;
            }
        }
    }
    hits
}

fn search_hit_view(hit: &SearchHit) -> (taskbar::SearchResultKind, String) {
    match hit {
        SearchHit::App(kind) => (taskbar::SearchResultKind::App, String::from(kind.label())),
        SearchHit::File(path) => (taskbar::SearchResultKind::File, path.clone()),
    }
}

fn parent_dir(path: &str) -> String {
    match path.rfind('/') {
        Some(0) => String::from("/"),
        Some(idx) => path[..idx].to_string(),
        None => String::from("/"),
    }
}

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
    /// z-order: the last entry is topmost and holds keyboard focus. Always
    /// holds `current_desktop`'s windows -- the other virtual desktops'
    /// windows live in `other_desktops` until switched to (see
    /// `switch_desktop`), so every existing piece of code that reads
    /// `gui.windows` keeps working unchanged and is automatically
    /// per-desktop.
    windows: Vec<Window>,
    /// The other virtual desktops' window lists. `other_desktops[current_desktop]`
    /// is always empty/unused -- that desktop's real content is in `windows`.
    other_desktops: [Vec<Window>; taskbar::WORKSPACE_COUNT],
    current_desktop: usize,
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
    /// The Moon-button app launcher popup, open or closed.
    moon_menu: Option<ContextMenu>,
    /// A read-only tray popup (Network/Bluetooth/Battery info) -- any click
    /// just dismisses it, there's nothing to select.
    info_popup: Option<ContextMenu>,
    /// The user-icon's power menu (Reboot/Shutdown).
    user_menu: Option<ContextMenu>,
    /// The desktop's own right-click menu (New Folder / Change Wallpaper),
    /// separate from a window's right-click menu.
    desktop_menu: Option<ContextMenu>,
    desktop_last_click: Option<(desktop_icons::DesktopIcon, u64)>,
    notif_panel_open: bool,
    search_active: bool,
    search_query: String,
    search_results: Vec<SearchHit>,
    /// Gates every other input path while `true` -- only the login card's
    /// password field and its Log In button respond. Starts locked so the
    /// desktop never shows before a real credential check, and can be
    /// re-armed at runtime via the taskbar user menu's "Lock".
    locked: bool,
    login: login::LoginState,
}

impl GuiState {
    const fn new() -> Self {
        // Array literal length is hand-matched to `taskbar::WORKSPACE_COUNT`
        // (`Vec::new()` isn't `Copy`, so `[Vec::new(); N]` isn't available
        // in a const fn) -- the assertion below catches drift at compile
        // time if that constant ever changes.
        const _: () = assert!(taskbar::WORKSPACE_COUNT == 4);
        Self {
            windows: Vec::new(),
            other_desktops: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            current_desktop: 0,
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
            moon_menu: None,
            info_popup: None,
            user_menu: None,
            desktop_menu: None,
            desktop_last_click: None,
            notif_panel_open: false,
            search_active: false,
            search_query: String::new(),
            search_results: Vec::new(),
            locked: true,
            login: login::LoginState::new(),
        }
    }

    /// Focuses the app of `kind` if a window for it is already open
    /// (raising it to the top of z-order), or spawns a new one cascaded
    /// down from the last-opened window so it doesn't land exactly on top
    /// of an existing one.
    /// Switches to virtual desktop `target`: swaps `windows` (the active
    /// desktop's real content) out to its own slot in `other_desktops` and
    /// swaps `target`'s content in. Any in-progress drag/resize/menu state
    /// is cleared since it refers to window *indices* into the desktop
    /// being left, which are meaningless once `windows` holds a different
    /// desktop's list.
    fn switch_desktop(&mut self, target: usize) {
        if target == self.current_desktop || target >= taskbar::WORKSPACE_COUNT {
            return;
        }
        core::mem::swap(
            &mut self.windows,
            &mut self.other_desktops[self.current_desktop],
        );
        core::mem::swap(&mut self.windows, &mut self.other_desktops[target]);
        self.current_desktop = target;
        self.dragging = None;
        self.resizing = None;
        self.context_menu = None;
        self.snap_preview = None;
    }

    fn open_app(&mut self, kind: AppKind, screen_w: usize, screen_h: usize) {
        if let Some(idx) = self
            .windows
            .iter()
            .position(|w| w.title == kind.dock_title())
        {
            let was_minimized = self.windows[idx].minimized;
            let icon = taskbar_icon_index_for(&self.windows, idx);
            let mut w = self.windows.remove(idx);
            w.minimized = false;
            if was_minimized {
                let now = crate::sched::ticks();
                let to = (w.x, w.y, w.w, w.h);
                let from = icon
                    .map(|i| taskbar::app_icon_rect(i, screen_h))
                    .unwrap_or(to);
                w.anim = Some(window::WindowAnim::new(
                    window::AnimKind::Unminimize,
                    now,
                    window::MINIMIZE_ANIM_TICKS,
                    from,
                    to,
                ));
            }
            self.windows.push(w);
            return;
        }

        let (ax, ay, _, _) = content_area(screen_w, screen_h);
        let cascade = (self.windows.len() as i32 % 6) * 24;
        let (w, h) = kind.default_size();
        let id = self.next_id;
        self.next_id += 1;
        let (x, y) = (ax + cascade, ay + cascade);
        let now = crate::sched::ticks();
        self.windows.push(Window {
            id,
            x,
            y,
            w,
            h,
            title: String::from(kind.dock_title()),
            content: kind.new_content(),
            minimized: false,
            maximized: None,
            opened_at: now,
            closing_since: None,
            anim: Some(open_anim(now, x, y, w, h)),
        });
    }
}

/// A small rect centered on `(x, y, w, h)` -- the "from" an opening window
/// grows out of, and the "to" a closing window shrinks into, so both read
/// as real geometry transitions rather than a flat fade.
fn centered_shrink(x: i32, y: i32, w: u32, h: u32) -> (i32, i32, u32, u32) {
    let cx = x + w as i32 / 2;
    let cy = y + h as i32 / 2;
    let sw = (w / 3).max(window::MIN_W / 2);
    let sh = (h / 3).max(window::MIN_H / 2);
    (cx - sw as i32 / 2, cy - sh as i32 / 2, sw, sh)
}

fn open_anim(now: u64, x: i32, y: i32, w: u32, h: u32) -> window::WindowAnim {
    window::WindowAnim::new(
        window::AnimKind::Open,
        now,
        window::OPEN_ANIM_TICKS,
        centered_shrink(x, y, w, h),
        (x, y, w, h),
    )
}

fn close_anim(now: u64, x: i32, y: i32, w: u32, h: u32) -> window::WindowAnim {
    window::WindowAnim::new(
        window::AnimKind::Close,
        now,
        window::CLOSE_ANIM_TICKS,
        (x, y, w, h),
        centered_shrink(x, y, w, h),
    )
}

/// The icon index (in the same order the taskbar draws its center row)
/// window `windows[idx]` currently occupies, if it has one -- used to
/// animate minimize/unminimize toward/from the real icon position.
fn taskbar_icon_index_for(windows: &[Window], idx: usize) -> Option<usize> {
    let slots = taskbar_app_targets(windows);
    let title = &windows[idx].title;
    slots.iter().position(|s| match s {
        TaskbarSlot::Pinned(kind) => kind.dock_title() == title,
        TaskbarSlot::Window(i) => *i == idx,
    })
}

static GUI: Mutex<GuiState> = Mutex::new(GuiState::new());

/// App-open requests queued by widgets that can't reach `GUI` directly
/// (currently just the Moon AI assistant, via `request_open`) -- drained
/// after every keystroke in `on_key`.
static PENDING_OPEN: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// Asks the window manager to open (or focus, if already open) the named
/// app on the next keystroke drain. `key` is one of the short strings
/// `AppKind::from_key` understands (`"settings"`, `"files"`, ...); an
/// unrecognized key is silently dropped when drained.
pub fn request_open(key: &'static str) {
    PENDING_OPEN.lock().push(key);
}

fn screen_size() -> (usize, usize) {
    framebuffer::with(|c| (c.width(), c.height())).unwrap_or((800, 600))
}

/// The desktop's usable content area: clear of the bottom taskbar and the
/// right-side widget panel (there's no top bar or left dock anymore -- both
/// folded into the taskbar). Used for maximize, edge-snap, and window
/// placement.
fn content_area(screen_w: usize, screen_h: usize) -> (i32, i32, u32, u32) {
    let x = 8;
    let y = 8;
    let w = screen_w as u32 - desktop_widgets::PANEL_W - 24;
    let h = screen_h as u32 - taskbar::HEIGHT - 16;
    (x, y, w, h)
}

pub fn init() {
    let (screen_w, screen_h) = screen_size();

    let mut gui = GUI.lock();
    gui.cursor_x = (screen_w / 2) as i32;
    gui.cursor_y = (screen_h / 2) as i32;
    gui.stars = desktop::generate_stars(screen_w, screen_h, 150);

    // The right side is left open for the calendar/system-monitor desktop
    // widgets; the bottom is left open for the taskbar.
    let content_top = 16;
    let content_left = 16;
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
        anim: Some(open_anim(now, content_left, content_top, 460, 260)),
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
        anim: Some(open_anim(
            now,
            content_left,
            content_top + 260 + 40,
            300,
            150,
        )),
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
        anim: Some(open_anim(now, second_col, content_top, 420, 260)),
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
        anim: Some(open_anim(now, second_col, content_top + 260 + 40, 420, 150)),
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
    let (screen_w, screen_h) = screen_size();
    {
        let mut gui = GUI.lock();
        if gui.locked {
            if login::handle_char(&mut gui.login, ch) {
                gui.locked = false;
            }
            drop(gui);
            redraw();
            return;
        }
        if gui.search_active {
            match ch {
                b'\n' | b'\r' => {
                    if !gui.search_results.is_empty() {
                        activate_search_hit(&mut gui, screen_w, screen_h, 0);
                    }
                }
                0x08 => {
                    gui.search_query.pop();
                    gui.search_results = run_search(&gui.search_query);
                }
                0x20..=0x7E => {
                    gui.search_query.push(ch as char);
                    gui.search_results = run_search(&gui.search_query);
                }
                _ => {}
            }
        } else if let Some(top) = gui.windows.last_mut() {
            top.handle_char(ch);
        }
    }
    drain_pending_opens();
    redraw();
}

/// Runs whatever `search_results[index]` points to (opening an app, or the
/// File Manager navigated to a found path's parent directory), then closes
/// the search box. Shared by pressing Enter and clicking a result row.
fn activate_search_hit(gui: &mut GuiState, screen_w: usize, screen_h: usize, index: usize) {
    if let Some(hit) = gui.search_results.get(index) {
        match hit {
            SearchHit::App(kind) => gui.open_app(*kind, screen_w, screen_h),
            SearchHit::File(path) => open_files_at(gui, screen_w, screen_h, parent_dir(path)),
        }
    }
    gui.search_active = false;
    gui.search_query.clear();
    gui.search_results.clear();
}

/// Focuses the existing File Manager window (navigating it to `dir`) or
/// spawns a new one already there. Shared by the search box's "jump to this
/// file's folder" result and the desktop's Home/Downloads/Documents/
/// Pictures/Music/Trash icons.
fn open_files_at(gui: &mut GuiState, screen_w: usize, screen_h: usize, dir: String) {
    if let Some(idx) = gui
        .windows
        .iter()
        .position(|w| w.title == AppKind::Files.dock_title())
    {
        let mut w = gui.windows.remove(idx);
        w.minimized = false;
        w.content = WindowContent::Files(FileManagerState::new_at(dir));
        gui.windows.push(w);
    } else {
        let (ax, ay, _, _) = content_area(screen_w, screen_h);
        let id = gui.next_id;
        gui.next_id += 1;
        let (w, h) = AppKind::Files.default_size();
        let now = crate::sched::ticks();
        gui.windows.push(Window {
            id,
            x: ax,
            y: ay,
            w,
            h,
            title: String::from(AppKind::Files.dock_title()),
            content: WindowContent::Files(FileManagerState::new_at(dir)),
            minimized: false,
            maximized: None,
            opened_at: now,
            closing_since: None,
            anim: Some(open_anim(now, ax, ay, w, h)),
        });
    }
}

/// Opens/focuses whatever `request_open` queued while handling that
/// keystroke -- e.g. the Moon AI widget finishing a "open settings" command
/// on Enter. A no-op on ticks where nothing queued anything.
fn drain_pending_opens() {
    let pending: Vec<&'static str> = {
        let mut queue = PENDING_OPEN.lock();
        if queue.is_empty() {
            return;
        }
        queue.drain(..).collect()
    };
    let (screen_w, screen_h) = screen_size();
    let mut gui = GUI.lock();
    for key in pending {
        if let Some(kind) = AppKind::from_key(key) {
            gui.open_app(kind, screen_w, screen_h);
        }
    }
}

/// Called from the keyboard IRQ handler for non-character keys (arrows,
/// Alt+Tab).
pub fn on_special_key(key: SpecialKey) {
    {
        let mut gui = GUI.lock();
        if gui.locked {
            drop(gui);
            redraw();
            return;
        }
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
                    || w.anim.is_some()
                    || now.saturating_sub(w.opened_at) < window::OPEN_ANIM_TICKS
            })
    };

    if animating {
        reap_closed_windows(now);
        advance_window_anims(now);
        redraw();
    } else if now.is_multiple_of(AMBIENT_REDRAW_TICKS) {
        redraw();
    }
}

fn reap_closed_windows(now: u64) {
    let mut gui = GUI.lock();
    gui.windows.retain(|w| !w.close_animation_done(now));
}

/// Clears any window's `anim` once it's finished -- important for more than
/// tidiness: `on_tick`'s `animating` check (above) treats `anim.is_some()`
/// as "still animating" so it can redraw every tick instead of only every
/// `AMBIENT_REDRAW_TICKS`, so a finished animation left dangling here would
/// force full-rate redraws forever. A finished `Minimize` also flips
/// `minimized` to `true` here -- the window keeps rendering (shrinking
/// toward its taskbar icon) with `minimized` still `false` until this
/// fires.
fn advance_window_anims(now: u64) {
    let mut gui = GUI.lock();
    for w in gui.windows.iter_mut() {
        let Some(anim) = &w.anim else { continue };
        if !anim.done(now) {
            continue;
        }
        if anim.kind == window::AnimKind::Minimize {
            w.minimized = true;
        }
        w.anim = None;
    }
}

/// Called from the mouse IRQ handler with a decoded packet. `dy` follows the
/// PS/2 convention (positive = up), so it's subtracted, not added, to reach
/// screen-space (positive = down).
pub fn on_mouse(dx: i32, dy: i32, left: bool, right: bool, _middle: bool) {
    let (screen_w, screen_h) = screen_size();
    let now = crate::sched::ticks();

    {
        let mut gui = GUI.lock();
        let (sdx, sdy) = (mouse_speed::scale(dx), mouse_speed::scale(dy));
        gui.cursor_x = (gui.cursor_x + sdx).clamp(0, screen_w as i32 - 1);
        gui.cursor_y = (gui.cursor_y - sdy).clamp(0, screen_h as i32 - 1);
        let (cx, cy) = (gui.cursor_x, gui.cursor_y);

        if gui.locked {
            if left
                && !gui.left_was_down
                && login::handle_click(&mut gui.login, cx, cy, screen_w, screen_h)
            {
                gui.locked = false;
            }
            gui.left_was_down = left;
            gui.right_was_down = right;
            drop(gui);
            redraw();
            return;
        }

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

        // The notification bell's dropdown: a click on its header row
        // (category filter / Clear all) is handled here and consumed,
        // before the taskbar's own click handling below (which is what
        // actually opens/closes the panel via the bell icon).
        if left && !gui.left_was_down && gui.notif_panel_open {
            let panel_h = notifications::panel_height();
            let panel_w = notifications::panel_width();
            let panel_x = screen_w as i32 - 16 - panel_w;
            let panel_y = taskbar::bar_top(screen_h) - 8 - panel_h;
            if (panel_x..panel_x + panel_w).contains(&cx)
                && (panel_y..panel_y + panel_h).contains(&cy)
            {
                match notifications::header_hit(cx - panel_x, cy - panel_y) {
                    Some(true) => notifications::cycle_category_filter(),
                    Some(false) => notifications::clear_history(),
                    None => {}
                }
                gui.left_was_down = left;
                gui.right_was_down = right;
                drop(gui);
                redraw();
                return;
            }
        }

        // Same capture-the-next-click pattern as the context menu, for the
        // Moon-button app launcher.
        if left && !gui.left_was_down {
            if let Some(menu) = gui.moon_menu.take() {
                let row = (cy - menu.y) / MENU_ROW_H;
                if cx >= menu.x
                    && cx < menu.x + MENU_W
                    && row >= 0
                    && (row as usize) < menu.items.len()
                {
                    let action = menu.items[row as usize].1 as usize;
                    if let Some(kind) = AppKind::ALL.get(action).copied() {
                        gui.open_app(kind, screen_w, screen_h);
                    }
                }
                gui.left_was_down = left;
                gui.right_was_down = right;
                drop(gui);
                redraw();
                return;
            }
            // Read-only info popups (Network/Bluetooth/Battery tray icons):
            // any click just dismisses, there's nothing to select.
            if gui.info_popup.take().is_some() {
                gui.left_was_down = left;
                gui.right_was_down = right;
                drop(gui);
                redraw();
                return;
            }
            if let Some(menu) = gui.user_menu.take() {
                let row = (cy - menu.y) / MENU_ROW_H;
                if cx >= menu.x
                    && cx < menu.x + MENU_W
                    && row >= 0
                    && (row as usize) < menu.items.len()
                {
                    match menu.items[row as usize].1 {
                        0 => crate::power::reboot(),
                        1 => crate::power::shutdown(),
                        2 => {
                            gui.locked = true;
                            gui.login.lock();
                        }
                        _ => {}
                    }
                }
                gui.left_was_down = left;
                gui.right_was_down = right;
                drop(gui);
                redraw();
                return;
            }
            if let Some(menu) = gui.desktop_menu.take() {
                let row = (cy - menu.y) / MENU_ROW_H;
                if cx >= menu.x
                    && cx < menu.x + MENU_W
                    && row >= 0
                    && (row as usize) < menu.items.len()
                {
                    match menu.items[row as usize].1 {
                        0 => {
                            let mut root = crate::fs::root().lock();
                            let mut n = 1u32;
                            loop {
                                let path =
                                    alloc::format!("{}/New Folder {}", desktop_icons::HOME_DIR, n);
                                if !root.exists(&path) {
                                    root.mkdir(&path);
                                    break;
                                }
                                n += 1;
                            }
                        }
                        1 => desktop::cycle_style(),
                        _ => {}
                    }
                }
                gui.left_was_down = left;
                gui.right_was_down = right;
                drop(gui);
                redraw();
                return;
            }
        }

        // Desktop icons (Home/Downloads/.../Trash, app shortcuts) and the
        // desktop's own right-click menu, both only reachable on the bare
        // desktop background -- not over the taskbar or any window.
        let over_desktop =
            !taskbar::bar_contains(cx, cy, screen_h) && hit_test(&gui.windows, cx, cy).is_none();

        if right && !gui.right_was_down && over_desktop && desktop_icons::icon_at(cx, cy).is_none()
        {
            gui.desktop_menu = Some(ContextMenu {
                x: cx,
                y: cy,
                items: alloc::vec![
                    (String::from("New Folder"), 0),
                    (alloc::format!("Wallpaper: {}", desktop::style_name()), 1),
                ],
            });
        }

        if left && !gui.left_was_down && over_desktop {
            gui.desktop_menu = None;
            if let Some(icon) = desktop_icons::icon_at(cx, cy) {
                let is_double = gui
                    .desktop_last_click
                    .is_some_and(|(last, t)| last == icon && now.saturating_sub(t) < 40);
                gui.desktop_last_click = Some((icon, now));
                if is_double {
                    match icon.target_dir() {
                        Some(dir) => open_files_at(&mut gui, screen_w, screen_h, String::from(dir)),
                        None => {
                            let kind = if icon == desktop_icons::DesktopIcon::TerminalShortcut {
                                AppKind::Terminal
                            } else {
                                AppKind::Store
                            };
                            gui.open_app(kind, screen_w, screen_h);
                        }
                    }
                }
            }
        }

        // Taskbar clicks: handled entirely separately from window hit-
        // testing below, since the bar always sits above every app window.
        if left && !gui.left_was_down && taskbar::bar_contains(cx, cy, screen_h) {
            gui.context_menu = None;

            let clicked_bell = matches!(
                taskbar::tray_hit(cx, cy, screen_w, screen_h),
                Some(taskbar::TrayHit::Notifications)
            );
            if gui.notif_panel_open && !clicked_bell {
                gui.notif_panel_open = false;
            }

            if taskbar::logo_hit(cx, cy, screen_h) {
                let items = AppKind::ALL
                    .iter()
                    .enumerate()
                    .map(|(i, kind)| (String::from(kind.label()), i as u32))
                    .collect();
                let menu_h = AppKind::ALL.len() as i32 * MENU_ROW_H;
                gui.moon_menu = Some(ContextMenu {
                    x: 4,
                    y: taskbar::bar_top(screen_h) - menu_h,
                    items,
                });
            } else if taskbar::search_hit(cx, cy, screen_h) {
                gui.search_active = true;
                gui.search_results = run_search(&gui.search_query);
            } else if let Some(row) =
                taskbar::search_result_row_at(cx, cy, screen_h, gui.search_results.len())
            {
                activate_search_hit(&mut gui, screen_w, screen_h, row);
            } else if taskbar::ai_hit(cx, cy, screen_h) {
                gui.open_app(AppKind::MoonAi, screen_w, screen_h);
            } else if let Some(target) = taskbar::workspace_hit(cx, cy, screen_h) {
                gui.switch_desktop(target);
            } else if let Some(slot) = {
                let slots = taskbar_app_targets(&gui.windows);
                taskbar::app_icon_index_at(cx, cy, screen_h, slots.len()).map(|idx| slots[idx])
            } {
                match slot {
                    TaskbarSlot::Pinned(kind) => gui.open_app(kind, screen_w, screen_h),
                    TaskbarSlot::Window(i) => {
                        let was_minimized = gui.windows.get(i).is_some_and(|w| w.minimized);
                        if was_minimized {
                            let icon = taskbar_icon_index_for(&gui.windows, i);
                            if let Some(w) = gui.windows.get_mut(i) {
                                w.minimized = false;
                                let to = (w.x, w.y, w.w, w.h);
                                let from = icon
                                    .map(|idx| taskbar::app_icon_rect(idx, screen_h))
                                    .unwrap_or(to);
                                w.anim = Some(window::WindowAnim::new(
                                    window::AnimKind::Unminimize,
                                    now,
                                    window::MINIMIZE_ANIM_TICKS,
                                    from,
                                    to,
                                ));
                            }
                        }
                        let w = gui.windows.remove(i);
                        gui.windows.push(w);
                    }
                }
            } else if let Some(hit) = taskbar::tray_hit(cx, cy, screen_w, screen_h) {
                match hit {
                    taskbar::TrayHit::Notifications => {
                        gui.notif_panel_open = !gui.notif_panel_open;
                        if gui.notif_panel_open {
                            notifications::mark_all_read();
                        }
                    }
                    taskbar::TrayHit::Volume => {
                        let next = match crate::audio::volume() {
                            0 => 100,
                            v if v <= 25 => 0,
                            v if v <= 50 => 25,
                            v if v <= 75 => 50,
                            _ => 75,
                        };
                        crate::audio::set_volume(next);
                        crate::audio::beep();
                    }
                    taskbar::TrayHit::User => {
                        gui.user_menu = Some(ContextMenu {
                            x: screen_w as i32 - 140,
                            y: taskbar::bar_top(screen_h) - 3 * MENU_ROW_H,
                            items: alloc::vec![
                                (String::from("Lock"), 2),
                                (String::from("Reboot"), 0),
                                (String::from("Shutdown"), 1),
                            ],
                        });
                    }
                    taskbar::TrayHit::Network => {
                        let items = alloc::vec![
                            (
                                if crate::net::is_up() {
                                    alloc::format!(
                                        "IP: {}",
                                        crate::net::format_ip(crate::net::our_ip())
                                    )
                                } else {
                                    String::from("No NIC detected")
                                },
                                0
                            ),
                            (
                                alloc::format!(
                                    "Gateway: {}",
                                    crate::net::format_ip(crate::net::gateway())
                                ),
                                0
                            ),
                        ];
                        gui.info_popup = Some(ContextMenu {
                            x: screen_w as i32 - 220,
                            y: taskbar::bar_top(screen_h) - 2 * MENU_ROW_H,
                            items,
                        });
                    }
                    taskbar::TrayHit::Bluetooth => {
                        gui.info_popup = Some(ContextMenu {
                            x: screen_w as i32 - 220,
                            y: taskbar::bar_top(screen_h) - MENU_ROW_H,
                            items: alloc::vec![(String::from("No Bluetooth adapter detected"), 0)],
                        });
                    }
                    taskbar::TrayHit::Battery => {
                        gui.info_popup = Some(ContextMenu {
                            x: screen_w as i32 - 220,
                            y: taskbar::bar_top(screen_h) - MENU_ROW_H,
                            items: alloc::vec![(
                                String::from("Running on AC power (no battery)"),
                                0
                            )],
                        });
                    }
                    taskbar::TrayHit::Cpu | taskbar::TrayHit::Ram | taskbar::TrayHit::NetSpeed => {}
                    taskbar::TrayHit::Clock => {}
                }
            } else if gui.search_active {
                gui.search_active = false;
            }

            gui.left_was_down = left;
            gui.right_was_down = right;
            drop(gui);
            redraw();
            return;
        }

        if left
            && !gui.left_was_down
            && gui.search_active
            && !taskbar::bar_contains(cx, cy, screen_h)
        {
            gui.search_active = false;
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

            if let Some(idx) = hit_test(&gui.windows, cx, cy) {
                let w = &gui.windows[idx];
                if let Some(btn) = w.button_at(cx, cy) {
                    match btn {
                        TitleButton::Close => {
                            let (x, y, w, h) = (w.x, w.y, w.w, w.h);
                            gui.windows[idx].begin_closing(now);
                            gui.windows[idx].anim = Some(close_anim(now, x, y, w, h));
                        }
                        TitleButton::Minimize => {
                            let icon = taskbar_icon_index_for(&gui.windows, idx);
                            let (x, y, w, h) = {
                                let t = &gui.windows[idx];
                                (t.x, t.y, t.w, t.h)
                            };
                            let to = icon
                                .map(|i| taskbar::app_icon_rect(i, screen_h))
                                .unwrap_or((x, screen_h as i32, 4, 4));
                            gui.windows[idx].anim = Some(window::WindowAnim::new(
                                window::AnimKind::Minimize,
                                now,
                                window::MINIMIZE_ANIM_TICKS,
                                (x, y, w, h),
                                to,
                            ));
                        }
                        TitleButton::Maximize => {
                            let (from_x, from_y, from_w, from_h) = (w.x, w.y, w.w, w.h);
                            let (ax, ay, aw, ah) = content_area(screen_w, screen_h);
                            gui.windows[idx].toggle_maximize(ax, ay, aw, ah);
                            let target = &gui.windows[idx];
                            let to = (target.x, target.y, target.w, target.h);
                            gui.windows[idx].anim = Some(window::WindowAnim::new(
                                window::AnimKind::Resize,
                                now,
                                window::RESIZE_ANIM_TICKS,
                                (from_x, from_y, from_w, from_h),
                                to,
                            ));
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
            let was_window_drag_or_resize = gui.dragging.is_some() || gui.resizing.is_some();
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

            // A content-area drag-and-drop release: only when this button-
            // up isn't the tail end of a title-bar drag/resize, and only on
            // the exact press->release transition (not every tick the
            // button happens to be up).
            if !was_window_drag_or_resize && gui.left_was_down {
                if let Some(idx) = hit_test(&gui.windows, cx, cy) {
                    let w = &gui.windows[idx];
                    if !w.title_bar_contains(cx, cy) {
                        let (lx, ly) = (cx - w.x, cy - w.y - window::TITLE_BAR_HEIGHT);
                        gui.windows[idx].handle_drag_release(lx, ly);
                    }
                }
            }
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
    if cy <= EDGE {
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

    // The login/lock screen replaces the entire desktop -- the animated
    // wallpaper still plays behind it (so the OS still reads as "alive"
    // while locked), but no window, taskbar, or desktop-icon content ever
    // renders (or receives input, see on_key/on_mouse) until unlocked.
    if gui.locked {
        desktop::render(&gui.stars, screen_w, screen_h, now);
        login::render(screen_w, screen_h, &gui.login);
        framebuffer::with(|c| {
            cursor::draw(c, gui.cursor_x, gui.cursor_y, true);
            c.present();
        });
        return;
    }

    desktop::render(&gui.stars, screen_w, screen_h, now);
    desktop_icons::render();
    desktop_widgets::render(screen_w as i32 - desktop_widgets::PANEL_W as i32 - 16, 16);

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
            c.blend_rect(sx, sy, sw, sh, theme::accent(), 60);
        });
    }

    // The taskbar is OS chrome, always drawn above every app window -- same
    // convention as a real desktop's menu bar/dock.
    let targets = taskbar_app_targets(&gui.windows);
    let app_icons: Vec<taskbar::AppIcon> = targets
        .iter()
        .map(|slot| taskbar_icon_view(slot, &gui.windows, focused_id))
        .collect();
    let search_results: Vec<(taskbar::SearchResultKind, String)> =
        gui.search_results.iter().map(search_hit_view).collect();
    let dt = crate::drivers::rtc::read();
    let stats = crate::memory::pmm::stats();
    let ram_pct = if stats.total_frames == 0 {
        0
    } else {
        (100 - (stats.free_frames * 100 / stats.total_frames)).min(100) as u8
    };
    let (tx_rate, rx_rate) = desktop_widgets::sample_net_rate();
    let tray = taskbar::TrayStats {
        cpu_pct: crate::sched::cpu_busy_percent(),
        ram_pct,
        net_tx_bps: tx_rate,
        net_rx_bps: rx_rate,
        net_connected: crate::net::is_up() && crate::net::our_ip() != crate::net::UNSPECIFIED_IP,
        bluetooth_present: crate::drivers::bluetooth::adapter_present(),
        volume_pct: crate::audio::volume(),
        audio_up: crate::audio::is_up(),
        unread_notifications: notifications::unread_count(),
        clock: alloc::format!("{:02}:{:02}:{:02}", dt.hour, dt.minute, dt.second),
        date: alloc::format!("{:04}-{:02}-{:02}", dt.year, dt.month, dt.day),
    };
    let mut occupied = [false; taskbar::WORKSPACE_COUNT];
    for (i, slot) in occupied.iter_mut().enumerate() {
        *slot = if i == gui.current_desktop {
            !gui.windows.is_empty()
        } else {
            !gui.other_desktops[i].is_empty()
        };
    }
    let workspace = taskbar::WorkspaceInfo {
        current: gui.current_desktop,
        occupied,
    };
    taskbar::render(
        screen_w,
        screen_h,
        &app_icons,
        &taskbar::SearchBoxState {
            active: gui.search_active,
            query: &gui.search_query,
            results: &search_results,
        },
        &tray,
        &workspace,
    );
    notifications::render(screen_w as i32 - 16, 16);
    if gui.notif_panel_open {
        notifications::render_history_panel(screen_w as i32 - 16, taskbar::bar_top(screen_h) - 8);
    }

    if let Some((_, menu)) = &gui.context_menu {
        render_context_menu(menu);
    }
    if let Some(menu) = &gui.moon_menu {
        render_context_menu(menu);
    }
    if let Some(menu) = &gui.user_menu {
        render_context_menu(menu);
    }
    if let Some(menu) = &gui.info_popup {
        render_context_menu(menu);
    }
    if let Some(menu) = &gui.desktop_menu {
        render_context_menu(menu);
    }

    framebuffer::with(|c| {
        let hovering_clickable = gui.dragging.is_none()
            && (taskbar::bar_contains(gui.cursor_x, gui.cursor_y, screen_h)
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
        c.glow_border(menu.x, menu.y, MENU_W as u32, h, theme::accent());
        c.blend_rect(menu.x, menu.y, MENU_W as u32, h, (0x14, 0x18, 0x24), 235);
        for (i, (label, _)) in menu.items.iter().enumerate() {
            let row_y = menu.y + i as i32 * MENU_ROW_H;
            c.draw_str_at(menu.x + 6, row_y + 4, label, (0xE0, 0xE0, 0xE0), None);
        }
    });
}
