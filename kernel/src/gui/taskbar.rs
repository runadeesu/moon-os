//! The unified bottom taskbar: Moon menu + search + AI button on the left,
//! pinned/running app icons in the center, and a system tray on the right.
//! Replaces the old separate top status bar and left dock.
//!
//! This module only renders and hit-tests against plain data its caller
//! (`gui::mod`) already computed -- it doesn't know about `AppKind` or
//! `GuiState` at all, so the same geometry math backs both drawing and
//! click dispatch with no risk of the two drifting apart.

use crate::framebuffer;
use alloc::format;
use alloc::string::String;

pub const HEIGHT: u32 = 46;
const LOGO_W: i32 = 46;
const SEARCH_W: i32 = 180;
const AI_W: i32 = 40;
const ZONE_GAP: i32 = 6;
pub const APP_ICON_W: i32 = 40;
const APP_ICON_GAP: i32 = 6;
const APP_ICON_MARGIN: i32 = 4;

/// The on-screen rect of the `index`-th center icon (same left-to-right
/// order `render`/`app_icon_index_at` use) -- the target a window's
/// minimize animation shrinks toward, and the source an unminimize
/// animation grows from.
pub fn app_icon_rect(index: usize, screen_h: usize) -> (i32, i32, u32, u32) {
    let top = bar_top(screen_h);
    let x = apps_x() + index as i32 * (APP_ICON_W + APP_ICON_GAP);
    let y = top + APP_ICON_MARGIN;
    let h = HEIGHT as i32 - APP_ICON_MARGIN * 2;
    (x, y, APP_ICON_W as u32, h as u32)
}

const TRAY_ICON_W: i32 = 30;
const TRAY_STAT_W: i32 = 46;
const TRAY_NET_W: i32 = 96;
const TRAY_CLOCK_W: i32 = 84;
const TRAY_GAP: i32 = 4;
const TRAY_PAD_RIGHT: i32 = 10;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TrayHit {
    Cpu,
    Ram,
    NetSpeed,
    Battery,
    Volume,
    Bluetooth,
    Network,
    Notifications,
    User,
    Clock,
}

/// Right-to-left tray layout -- the single source of truth both `render`
/// and `tray_hit` walk, so their geometry can never disagree.
const TRAY_ITEMS: [(TrayHit, i32); 10] = [
    (TrayHit::Clock, TRAY_CLOCK_W),
    (TrayHit::User, TRAY_ICON_W),
    (TrayHit::Notifications, TRAY_ICON_W),
    (TrayHit::Network, TRAY_ICON_W),
    (TrayHit::Bluetooth, TRAY_ICON_W),
    (TrayHit::Volume, TRAY_ICON_W),
    (TrayHit::Battery, TRAY_ICON_W),
    (TrayHit::Ram, TRAY_STAT_W),
    (TrayHit::Cpu, TRAY_STAT_W),
    (TrayHit::NetSpeed, TRAY_NET_W),
];

fn tray_rects(screen_w: usize) -> [(TrayHit, i32, i32); TRAY_ITEMS.len()] {
    let mut rects = [(TrayHit::Clock, 0, 0); TRAY_ITEMS.len()];
    let mut x = screen_w as i32 - TRAY_PAD_RIGHT;
    for (i, (item, w)) in TRAY_ITEMS.iter().enumerate() {
        x -= w;
        rects[i] = (*item, x, *w);
        x -= TRAY_GAP;
    }
    rects
}

pub fn bar_top(screen_h: usize) -> i32 {
    screen_h as i32 - HEIGHT as i32
}

pub fn bar_contains(_x: i32, y: i32, screen_h: usize) -> bool {
    y >= bar_top(screen_h)
}

pub fn logo_hit(x: i32, y: i32, screen_h: usize) -> bool {
    (0..LOGO_W).contains(&x) && y >= bar_top(screen_h)
}

fn search_x() -> i32 {
    LOGO_W + ZONE_GAP
}

pub fn search_hit(x: i32, y: i32, screen_h: usize) -> bool {
    (search_x()..search_x() + SEARCH_W).contains(&x) && y >= bar_top(screen_h)
}

const SEARCH_ROW_H: i32 = 18;

/// Hit-tests the search dropdown (which opens *upward* from the search box,
/// since the taskbar itself is at the bottom of the screen), returning the
/// result row index if any.
pub fn search_result_row_at(x: i32, y: i32, screen_h: usize, count: usize) -> Option<usize> {
    if count == 0 || !(search_x()..search_x() + SEARCH_W).contains(&x) {
        return None;
    }
    let top = bar_top(screen_h);
    if y >= top {
        return None;
    }
    let dropdown_top = top - count as i32 * SEARCH_ROW_H;
    if y < dropdown_top {
        return None;
    }
    let idx = ((y - dropdown_top) / SEARCH_ROW_H) as usize;
    if idx < count {
        Some(idx)
    } else {
        None
    }
}

fn ai_x() -> i32 {
    search_x() + SEARCH_W + ZONE_GAP
}

pub fn ai_hit(x: i32, y: i32, screen_h: usize) -> bool {
    (ai_x()..ai_x() + AI_W).contains(&x) && y >= bar_top(screen_h)
}

/// Number of virtual desktops/workspaces -- fixed rather than user-
/// configurable, same pragmatic scope as the theme accent presets.
pub const WORKSPACE_COUNT: usize = 4;
const WORKSPACE_BOX_W: i32 = 22;
const WORKSPACE_GAP: i32 = 4;

fn workspace_x() -> i32 {
    ai_x() + AI_W + ZONE_GAP
}

/// Which workspace box (if any) `(x, y)` lands on.
pub fn workspace_hit(x: i32, y: i32, screen_h: usize) -> Option<usize> {
    if y < bar_top(screen_h) {
        return None;
    }
    let rel = x - workspace_x();
    if rel < 0 {
        return None;
    }
    let stride = WORKSPACE_BOX_W + WORKSPACE_GAP;
    let idx = (rel / stride) as usize;
    if rel % stride < WORKSPACE_BOX_W && idx < WORKSPACE_COUNT {
        Some(idx)
    } else {
        None
    }
}

pub fn apps_x() -> i32 {
    workspace_x() + WORKSPACE_COUNT as i32 * (WORKSPACE_BOX_W + WORKSPACE_GAP) + ZONE_GAP * 2
}

/// Index into the caller's combined pinned+running icon list, in the same
/// left-to-right order `render` draws them in.
pub fn app_icon_index_at(x: i32, y: i32, screen_h: usize, count: usize) -> Option<usize> {
    if y < bar_top(screen_h) {
        return None;
    }
    let rel = x - apps_x();
    if rel < 0 {
        return None;
    }
    let stride = APP_ICON_W + APP_ICON_GAP;
    let idx = (rel / stride) as usize;
    if rel % stride < APP_ICON_W && idx < count {
        Some(idx)
    } else {
        None
    }
}

pub fn tray_hit(x: i32, y: i32, screen_w: usize, screen_h: usize) -> Option<TrayHit> {
    if y < bar_top(screen_h) {
        return None;
    }
    tray_rects(screen_w)
        .into_iter()
        .find(|(_, rx, rw)| (*rx..*rx + *rw).contains(&x))
        .map(|(item, _, _)| item)
}

/// One app icon the taskbar draws in its center zone: pinned apps always
/// appear, other open windows appear alongside them -- see `gui::mod`'s
/// `taskbar_app_icons` for how the combined, ordered list is built.
pub struct AppIcon {
    pub letter: char,
    pub running: bool,
    pub focused: bool,
}

pub enum SearchResultKind {
    App,
    File,
}

pub struct TrayStats {
    pub cpu_pct: u8,
    pub ram_pct: u8,
    pub net_tx_bps: u64,
    pub net_rx_bps: u64,
    pub net_connected: bool,
    pub bluetooth_present: bool,
    pub volume_pct: u8,
    pub audio_up: bool,
    pub unread_notifications: usize,
    pub clock: String,
    pub date: String,
}

fn short_rate(bytes_per_sec: u64) -> String {
    if bytes_per_sec >= 1_000_000 {
        format!("{}M", bytes_per_sec / 1_000_000)
    } else if bytes_per_sec >= 1_000 {
        format!("{}K", bytes_per_sec / 1_000)
    } else {
        format!("{}B", bytes_per_sec)
    }
}

fn draw_icon_box(
    c: &mut framebuffer::Console,
    rect: (i32, i32, i32, i32),
    label: &str,
    color: (u8, u8, u8),
    dim: bool,
) {
    let (x, y, w, h) = rect;
    let bg = if dim {
        (0x16, 0x18, 0x22)
    } else {
        (0x12, 0x28, 0x36)
    };
    c.fill_rect(x, y, w as u32, h as u32, bg);
    if !dim {
        c.glow_border(x, y, w as u32, h as u32, color);
    }
    let text_color = if dim { (0x5A, 0x5E, 0x6A) } else { color };
    let tx = x + (w - label.len() as i32 * 8) / 2;
    let ty = y + (h - 8) / 2;
    c.draw_str_at(tx, ty, label, text_color, None);
}

/// Bundles the search box's transient state into one param so `render`
/// doesn't grow past a reasonable argument count.
pub struct SearchBoxState<'a> {
    pub active: bool,
    pub query: &'a str,
    pub results: &'a [(SearchResultKind, String)],
}

/// Which workspace is active and which ones have any windows open at all --
/// real state, not decoration, so an empty workspace's box renders visibly
/// different from one with something in it.
pub struct WorkspaceInfo {
    pub current: usize,
    pub occupied: [bool; WORKSPACE_COUNT],
}

pub fn render(
    screen_w: usize,
    screen_h: usize,
    app_icons: &[AppIcon],
    search: &SearchBoxState,
    tray: &TrayStats,
    workspace: &WorkspaceInfo,
) {
    let search_active = search.active;
    let search_query = search.query;
    let search_results = search.results;
    let neon = super::theme::accent();
    let top = bar_top(screen_h);

    framebuffer::with(|c| {
        // A real frosted-glass bar: blurs whatever's actually behind it
        // (the wallpaper, or windows already drawn this frame) rather than
        // just laying a flat tint over it.
        c.frosted_glass_rect(0, top, screen_w as u32, HEIGHT, (0x0A, 0x0C, 0x18), 190);
        c.blend_rect(0, top - 1, screen_w as u32, 1, neon, 130);
        c.blend_rect(0, top - 2, screen_w as u32, 1, neon, 55);

        // Crescent Moon logo -- same shape as the old top bar's, a bright
        // disc with a smaller disc of the bar's own background punched out.
        let cy = top + HEIGHT as i32 / 2;
        c.fill_circle(18, cy, 9, (0xC8, 0xD8, 0xFF));
        c.fill_circle(22, cy - 3, 8, (0x0A, 0x0C, 0x18));
    });

    // Search box.
    let sx = search_x();
    let sy = top + 6;
    let sh = HEIGHT as i32 - 12;
    framebuffer::with(|c| {
        c.fill_rect(sx, sy, SEARCH_W as u32, sh as u32, (0x12, 0x14, 0x1E));
        if search_active {
            c.glow_border(sx, sy, SEARCH_W as u32, sh as u32, neon);
        }
        let label = if search_query.is_empty() && !search_active {
            String::from("Search...")
        } else {
            format!("{}_", search_query)
        };
        let max_chars = ((SEARCH_W - 12) / 8).max(1) as usize;
        let shown = if label.len() > max_chars {
            &label[label.len() - max_chars..]
        } else {
            label.as_str()
        };
        let color = if search_query.is_empty() && !search_active {
            (0x60, 0x64, 0x70)
        } else {
            (0xE0, 0xE0, 0xE0)
        };
        c.draw_str_at(sx + 6, sy + (sh - 8) / 2, shown, color, None);
    });

    // Search results dropdown, opening upward from the search box.
    if search_active && !search_results.is_empty() {
        let h = search_results.len() as i32 * SEARCH_ROW_H;
        let ry = top - h;
        framebuffer::with(|c| {
            c.glow_border(sx, ry, SEARCH_W as u32, h as u32, neon);
            c.frosted_glass_rect(sx, ry, SEARCH_W as u32, h as u32, (0x12, 0x16, 0x22), 210);
            for (i, (kind, label)) in search_results.iter().enumerate() {
                let row_y = ry + i as i32 * SEARCH_ROW_H;
                let tag = match kind {
                    SearchResultKind::App => "[app] ",
                    SearchResultKind::File => "[file]",
                };
                let max_chars = ((SEARCH_W - 12) / 8).max(1) as usize - 6;
                let text = if label.len() > max_chars {
                    &label[..max_chars]
                } else {
                    label.as_str()
                };
                c.draw_str_at(sx + 6, row_y + 5, tag, neon, None);
                c.draw_str_at(sx + 54, row_y + 5, text, (0xD8, 0xD8, 0xD8), None);
            }
        });
    }

    // AI button.
    let aix = ai_x();
    framebuffer::with(|c| {
        c.fill_rect(aix, sy, AI_W as u32, sh as u32, (0x12, 0x28, 0x36));
        c.glow_border(aix, sy, AI_W as u32, sh as u32, neon);
        c.draw_str_at(aix + 6, sy + (sh - 8) / 2, "AI", neon, None);
    });

    // Workspace switcher: one small numbered box per virtual desktop.
    let icon_h = HEIGHT as i32 - APP_ICON_MARGIN * 2;
    let icon_y = top + APP_ICON_MARGIN;
    let wx = workspace_x();
    for i in 0..WORKSPACE_COUNT {
        let x = wx + i as i32 * (WORKSPACE_BOX_W + WORKSPACE_GAP);
        let active = i == workspace.current;
        let occupied = workspace.occupied[i];
        framebuffer::with(|c| {
            let bg = if active {
                (0x0E, 0x3A, 0x50)
            } else {
                (0x14, 0x16, 0x20)
            };
            c.fill_rect(x, icon_y, WORKSPACE_BOX_W as u32, icon_h as u32, bg);
            if active {
                c.glow_border(x, icon_y, WORKSPACE_BOX_W as u32, icon_h as u32, neon);
            }
            let label = alloc::format!("{}", i + 1);
            let color = if active {
                neon
            } else if occupied {
                (0xC0, 0xC0, 0xC8)
            } else {
                (0x50, 0x54, 0x60)
            };
            c.draw_str_at(
                x + (WORKSPACE_BOX_W - 8) / 2,
                icon_y + (icon_h - 8) / 2,
                &label,
                color,
                None,
            );
        });
    }

    // Pinned + running app icons.
    let mut ix = apps_x();
    for icon in app_icons {
        framebuffer::with(|c| {
            let bg = if icon.focused {
                (0x0E, 0x3A, 0x50)
            } else if icon.running {
                (0x1A, 0x1E, 0x2A)
            } else {
                (0x12, 0x14, 0x1C)
            };
            c.fill_rect(ix, icon_y, APP_ICON_W as u32, icon_h as u32, bg);
            if icon.focused {
                c.glow_border(ix, icon_y, APP_ICON_W as u32, icon_h as u32, neon);
            }
            let color = if icon.running {
                (0xE0, 0xE0, 0xE0)
            } else {
                (0x80, 0x84, 0x90)
            };
            let mut buf = [0u8; 4];
            let s = icon.letter.encode_utf8(&mut buf);
            c.draw_str_at(
                ix + (APP_ICON_W - 8) / 2,
                icon_y + (icon_h - 8) / 2 - 2,
                s,
                color,
                None,
            );
            if icon.running {
                c.fill_rect(ix + APP_ICON_W / 2 - 3, icon_y + icon_h - 4, 6, 2, neon);
            }
        });
        ix += APP_ICON_W + APP_ICON_GAP;
    }

    render_tray(screen_w, top, tray);
}

fn render_tray(screen_w: usize, top: i32, tray: &TrayStats) {
    let neon = super::theme::accent();
    let icon_h = HEIGHT as i32 - APP_ICON_MARGIN * 2;
    let icon_y = top + APP_ICON_MARGIN;

    for (item, x, w) in tray_rects(screen_w) {
        framebuffer::with(|c| match item {
            TrayHit::Cpu => draw_icon_box(
                c,
                (x, icon_y, w, icon_h),
                &format!("C{:>3}", tray.cpu_pct),
                neon,
                false,
            ),
            TrayHit::Ram => draw_icon_box(
                c,
                (x, icon_y, w, icon_h),
                &format!("R{:>3}", tray.ram_pct),
                neon,
                false,
            ),
            TrayHit::NetSpeed => draw_icon_box(
                c,
                (x, icon_y, w, icon_h),
                &format!(
                    "U{} D{}",
                    short_rate(tray.net_tx_bps),
                    short_rate(tray.net_rx_bps)
                ),
                neon,
                !tray.net_connected,
            ),
            TrayHit::Battery => {
                draw_icon_box(c, (x, icon_y, w, icon_h), "AC", (0x70, 0xD8, 0x90), false)
            }
            TrayHit::Volume => draw_icon_box(
                c,
                (x, icon_y, w, icon_h),
                &format!("{}", tray.volume_pct),
                neon,
                !tray.audio_up,
            ),
            TrayHit::Bluetooth => draw_icon_box(
                c,
                (x, icon_y, w, icon_h),
                "BT",
                (0x60, 0xA0, 0xE8),
                !tray.bluetooth_present,
            ),
            TrayHit::Network => draw_icon_box(
                c,
                (x, icon_y, w, icon_h),
                "NT",
                (0x50, 0xE8, 0x90),
                !tray.net_connected,
            ),
            TrayHit::Notifications => {
                let label = if tray.unread_notifications > 0 {
                    format!("N{}", tray.unread_notifications.min(9))
                } else {
                    String::from("N")
                };
                draw_icon_box(
                    c,
                    (x, icon_y, w, icon_h),
                    &label,
                    neon,
                    tray.unread_notifications == 0,
                );
            }
            TrayHit::User => draw_icon_box(c, (x, icon_y, w, icon_h), "R", neon, false),
            TrayHit::Clock => {
                c.fill_rect(x, icon_y, w as u32, icon_h as u32, (0x12, 0x14, 0x1E));
                c.draw_str_at(x + 4, icon_y + 4, &tray.clock, (0xE0, 0xE0, 0xE0), None);
                c.draw_str_at(x + 4, icon_y + 4 + 12, &tray.date, (0x90, 0x94, 0xA0), None);
            }
        });
    }
}
