//! Desktop icons: Home/Downloads/Documents/Pictures/Music/Trash folders
//! (real RAMFS directories, created at boot in `main.rs`) plus two app
//! shortcuts, arranged in a single column on the desktop background layer
//! -- below every window in z-order, same as a real desktop. Double-click
//! opens the folder (in File Manager, navigated straight there) or the app;
//! there's no drag-to-reposition yet, so the grid position is fixed rather
//! than persisted.

use crate::framebuffer;

pub const HOME_DIR: &str = "/home";
pub const DOWNLOADS_DIR: &str = "/home/Downloads";
pub const DOCUMENTS_DIR: &str = "/home/Documents";
pub const PICTURES_DIR: &str = "/home/Pictures";
pub const MUSIC_DIR: &str = "/home/Music";

const CELL_W: i32 = 76;
const CELL_H: i32 = 74;
const COL_X: i32 = 12;
const START_Y: i32 = 12;
const ICON_SIZE: i32 = 44;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DesktopIcon {
    Home,
    Downloads,
    Documents,
    Pictures,
    Music,
    Trash,
    TerminalShortcut,
    StoreShortcut,
}

pub const ALL: [DesktopIcon; 8] = [
    DesktopIcon::Home,
    DesktopIcon::Downloads,
    DesktopIcon::Documents,
    DesktopIcon::Pictures,
    DesktopIcon::Music,
    DesktopIcon::Trash,
    DesktopIcon::TerminalShortcut,
    DesktopIcon::StoreShortcut,
];

impl DesktopIcon {
    pub fn label(self) -> &'static str {
        match self {
            DesktopIcon::Home => "Home",
            DesktopIcon::Downloads => "Downloads",
            DesktopIcon::Documents => "Documents",
            DesktopIcon::Pictures => "Pictures",
            DesktopIcon::Music => "Music",
            DesktopIcon::Trash => "Trash",
            DesktopIcon::TerminalShortcut => "Terminal",
            DesktopIcon::StoreShortcut => "Moon Store",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            DesktopIcon::Home => "Ho",
            DesktopIcon::Downloads => "Dl",
            DesktopIcon::Documents => "Dc",
            DesktopIcon::Pictures => "Pi",
            DesktopIcon::Music => "Mu",
            DesktopIcon::Trash => "Tr",
            DesktopIcon::TerminalShortcut => ">_",
            DesktopIcon::StoreShortcut => "St",
        }
    }

    /// The RAMFS path a folder icon opens File Manager at. `None` for the
    /// two app shortcuts, which open an app instead (handled by the caller).
    pub fn target_dir(self) -> Option<&'static str> {
        match self {
            DesktopIcon::Home => Some(HOME_DIR),
            DesktopIcon::Downloads => Some(DOWNLOADS_DIR),
            DesktopIcon::Documents => Some(DOCUMENTS_DIR),
            DesktopIcon::Pictures => Some(PICTURES_DIR),
            DesktopIcon::Music => Some(MUSIC_DIR),
            DesktopIcon::Trash => Some(super::widgets::files::TRASH_DIR),
            DesktopIcon::TerminalShortcut | DesktopIcon::StoreShortcut => None,
        }
    }
}

fn rect_for(index: usize) -> (i32, i32, i32, i32) {
    let y = START_Y + index as i32 * CELL_H;
    (COL_X, y, CELL_W - 8, ICON_SIZE + 22)
}

pub fn icon_at(x: i32, y: i32) -> Option<DesktopIcon> {
    for (i, icon) in ALL.iter().enumerate() {
        let (ix, iy, iw, ih) = rect_for(i);
        if (ix..ix + iw).contains(&x) && (iy..iy + ih).contains(&y) {
            return Some(*icon);
        }
    }
    None
}

pub fn render() {
    let neon = super::theme::accent();
    for (i, icon) in ALL.iter().enumerate() {
        let (x, y, _, _) = rect_for(i);
        framebuffer::with(|c| {
            c.blend_rect(
                x,
                y,
                ICON_SIZE as u32,
                ICON_SIZE as u32,
                (0x10, 0x18, 0x28),
                150,
            );
            c.glow_border(x, y, ICON_SIZE as u32, ICON_SIZE as u32, neon);
            let glyph = icon.glyph();
            c.draw_str_at(
                x + (ICON_SIZE - glyph.len() as i32 * 8) / 2,
                y + (ICON_SIZE - 8) / 2,
                glyph,
                (0xE0, 0xE0, 0xE0),
                None,
            );
            let label = icon.label();
            let label_x = x + (CELL_W - 8 - label.len() as i32 * 8) / 2;
            c.draw_str_at(
                label_x.max(x - 8),
                y + ICON_SIZE + 4,
                label,
                (0xC0, 0xC8, 0xE0),
                None,
            );
        });
    }
}
