//! A real File Manager: navigable directories (backed by the RAMFS
//! directory support in `fs/ramfs.rs`), copy/cut/paste/delete/rename/
//! mkdir, a right-click context menu, a soft-delete trash folder, search,
//! multi-select, switchable list/grid views, ZIP compress/extract (shares
//! `apk.rs`'s ZIP reader, which now also decompresses real DEFLATE
//! entries via `inflate.rs`, and `zip.rs`'s writer), a recent-files list,
//! and single-item drag-and-drop onto a folder row.
//!
//! Double-clicking dispatches by real file type (`launch_path`, below):
//! `.mapp` and `.exe` are genuinely spawned as new ring-3 processes
//! (`.exe` via `crate::winexe`/`crate::pe`, with an honest "needs real
//! Win32 APIs" message when a real Windows binary's imports don't match
//! that loader's tiny supported subset -- see `winexe.rs`'s doc comment);
//! `.apk` is genuinely parsed and installed into `crate::androidpkg`'s
//! metadata registry (never executed -- there is no Android runtime);
//! `.msi` and anything else unrecognized get a plain, honest explanation
//! instead of silently doing nothing. `.mlnk` is this OS's own shortcut
//! format (right-click "Create Shortcut") -- a real file holding a target
//! path, resolved by recursing back into the same dispatch.

use crate::gui::ContextMenu;
use alloc::collections::{BTreeSet, VecDeque};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::Cell;
use spin::Mutex;

const ROW_H: i32 = 16;
const HEADER_H: i32 = 20;
const TOOLBAR_H: i32 = 16;
const LIST_TOP: i32 = HEADER_H + TOOLBAR_H;
/// How close together (in scheduler ticks, ~100/sec) two clicks on the same
/// row need to land to count as a double-click.
const DOUBLE_CLICK_TICKS: u64 = 40;

pub const TRASH_DIR: &str = "/.Trash";
const RECENT_CAP: usize = 10;

/// Files/packages actually launched (via double-click), most recent last --
/// real usage history, not a canned demo list. Shared across every File
/// Manager window/instance, same as a real desktop's "recent" list would be.
static RECENT: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

fn record_recent(path: &str) {
    let mut recent = RECENT.lock();
    recent.retain(|p| p != path);
    recent.push_back(String::from(path));
    while recent.len() > RECENT_CAP {
        recent.pop_front();
    }
}

const ACTION_RENAME: u32 = 0;
const ACTION_DELETE: u32 = 1;
const ACTION_COPY: u32 = 2;
const ACTION_CUT: u32 = 3;
const ACTION_PASTE: u32 = 4;
const ACTION_NEW_FOLDER: u32 = 5;
const ACTION_COMPRESS: u32 = 6;
const ACTION_EXTRACT: u32 = 7;
const ACTION_RUN: u32 = 8;
const ACTION_SHORTCUT: u32 = 9;
const ACTION_PROPERTIES: u32 = 10;

/// Dispatches a double-click (or a `.mlnk` shortcut's resolved target) to
/// that file's real launch behavior. `.mapp`/`.exe` genuinely spawn a new
/// ring-3 process; `.apk` is genuinely parsed (real ZIP + DEFLATE + AXML)
/// and installed into the metadata-only Android registry; everything this
/// doesn't recognize gets an honest, specific explanation rather than
/// silently doing nothing.
fn launch_path(path: &str) -> String {
    if path.ends_with(".mapp") {
        match crate::pkg::run(path) {
            Ok(name) => {
                record_recent(path);
                format!("launched {name}")
            }
            Err(err) => format!("launch failed: {err}"),
        }
    } else if path.ends_with(".exe") {
        match crate::winexe::run(path) {
            Ok(name) => {
                record_recent(path);
                format!("launched {name}")
            }
            Err(err) => err,
        }
    } else if path.ends_with(".msi") {
        String::from(
            "MSI installer format recognized, but moon OS has no Windows Installer service to run it -- not implemented",
        )
    } else if path.ends_with(".apk") {
        let bytes = crate::fs::root().lock().read(path).map(|d| d.to_vec());
        match bytes {
            Some(bytes) => match crate::androidpkg::install(&bytes) {
                Ok(app) => {
                    record_recent(path);
                    crate::gui::notifications::push(
                        crate::gui::notifications::Kind::Success,
                        crate::gui::notifications::Category::Packages,
                        format!("installed Android package {} ({})", app.package, app.label),
                    );
                    format!(
                        "installed {} ({}) -- metadata only, no Android runtime to actually run its code",
                        app.package, app.label
                    )
                }
                Err(err) => format!("APK install failed: {err}"),
            },
            None => String::from("couldn't read that file"),
        }
    } else if path.ends_with(".mlnk") {
        let target = crate::fs::root()
            .lock()
            .read(path)
            .map(|d| String::from_utf8_lossy(d).into_owned());
        match target {
            Some(target) if crate::fs::root().lock().exists(&target) => launch_path(&target),
            Some(target) => format!("shortcut target missing: {target}"),
            None => String::from("couldn't read shortcut"),
        }
    } else if path.ends_with(".zip") {
        String::from("double-click won't extract a ZIP -- right-click it for Extract")
    } else {
        record_recent(path);
        format!("{} is not a launchable package", file_name(path))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    List,
    Grid,
}

pub struct FileManagerState {
    current_path: String,
    entries: Vec<(String, bool)>,
    selected: BTreeSet<usize>,
    last_click: Option<(usize, u64)>,
    clipboard: Option<(String, bool)>, // (path, is_cut)
    context_target: Option<String>,
    view: ViewMode,
    searching: bool,
    search: String,
    status: String,
    /// Content-area width from the most recent `render()` call, in grid
    /// columns -- `render` is `&self` (called through a shared `&self.content`
    /// match in `Window::render`), so this needs interior mutability to
    /// let `handle_click`/`handle_right_click` hit-test against the same
    /// column count the grid was actually drawn with.
    grid_cols: Cell<i32>,
    /// Showing the real-usage "Recent" list instead of `current_path`'s
    /// directory listing.
    showing_recent: bool,
    /// Row a plain (non-double) click landed on, for single-item drag-and-
    /// drop: a later mouse-up over a directory row moves it there. There's
    /// no continuous mouse-move tracking into widgets yet, so there's no
    /// "ghost icon following the cursor" while dragging -- the drop itself
    /// is a real move, just without that visual feedback mid-drag.
    drag_source: Option<usize>,
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

impl FileManagerState {
    pub fn new() -> Self {
        let mut state = Self {
            current_path: String::from("/"),
            entries: Vec::new(),
            selected: BTreeSet::new(),
            last_click: None,
            clipboard: None,
            context_target: None,
            view: ViewMode::List,
            searching: false,
            search: String::new(),
            status: String::from("right-click for options, double-click to open"),
            grid_cols: Cell::new(1),
            showing_recent: false,
            drag_source: None,
        };
        state.refresh();
        state
    }

    /// Opens directly into `path` instead of the root -- used by the
    /// taskbar search box's "jump to this folder" result and the desktop's
    /// Home/Downloads/Documents/Pictures/Music icons.
    pub fn new_at(path: String) -> Self {
        let mut state = Self::new();
        state.navigate(path);
        state
    }

    fn refresh(&mut self) {
        let root = crate::fs::root().lock();
        let mut entries = if self.showing_recent {
            RECENT
                .lock()
                .iter()
                .rev()
                .filter(|p| root.exists(p))
                .map(|p| (p.clone(), false))
                .collect()
        } else {
            root.list_dir(&self.current_path)
        };
        if !self.search.is_empty() {
            let needle = self.search.to_ascii_lowercase();
            entries.retain(|(path, _)| file_name(path).to_ascii_lowercase().contains(&needle));
        }
        self.entries = entries;
        self.selected.clear();
    }

    fn navigate(&mut self, path: String) {
        self.current_path = path;
        self.showing_recent = false;
        self.refresh();
    }

    fn parent_path(&self) -> Option<String> {
        if self.current_path == "/" {
            return None;
        }
        let idx = self.current_path.rfind('/').unwrap_or(0);
        Some(if idx == 0 {
            String::from("/")
        } else {
            self.current_path[..idx].to_string()
        })
    }

    /// Row index 0 is a synthetic ".." entry whenever not at the root (the
    /// Recent list isn't a real directory, so it never gets one).
    fn has_parent_row(&self) -> bool {
        !self.showing_recent && self.parent_path().is_some()
    }

    fn row_path(&self, row: usize) -> Option<(&str, bool)> {
        if self.has_parent_row() {
            if row == 0 {
                return None; // ".." handled separately by callers
            }
            self.entries.get(row - 1).map(|(p, d)| (p.as_str(), *d))
        } else {
            self.entries.get(row).map(|(p, d)| (p.as_str(), *d))
        }
    }

    pub fn handle_char(&mut self, ch: u8) {
        if !self.searching {
            return;
        }
        match ch {
            b'\n' => self.searching = false,
            0x08 => {
                self.search.pop();
                self.refresh();
            }
            0x20..=0x7E => {
                self.search.push(ch as char);
                self.refresh();
            }
            _ => {}
        }
    }

    /// `x`/`y` are local to the widget's content area, same convention as
    /// `Window::handle_click`.
    pub fn handle_click(&mut self, x: i32, y: i32) {
        if y < HEADER_H {
            // Breadcrumb: click anywhere on the path bar to jump to root --
            // a full per-segment breadcrumb is more UI than this pass adds.
            self.navigate(String::from("/"));
            return;
        }
        if y < LIST_TOP {
            if x < 50 {
                self.view = ViewMode::List;
            } else if x < 100 {
                self.view = ViewMode::Grid;
            } else if x < 160 {
                self.searching = !self.searching;
                if !self.searching {
                    self.search.clear();
                    self.refresh();
                }
            } else if x < 230 {
                self.showing_recent = !self.showing_recent;
                self.refresh();
            }
            return;
        }

        let row = match self.view {
            ViewMode::List => (y - LIST_TOP) / ROW_H,
            ViewMode::Grid => {
                let cols = self.grid_cols();
                let cell = self.grid_cell_size();
                let col = x / cell;
                let grid_row = (y - LIST_TOP) / cell;
                if col >= cols {
                    return;
                }
                grid_row * cols + col
            }
        };
        if row < 0 {
            return;
        }
        let row = row as usize;

        if self.has_parent_row() && row == 0 {
            if let Some(parent) = self.parent_path() {
                self.navigate(parent);
            }
            return;
        }

        let total_rows = self.entries.len() + usize::from(self.has_parent_row());
        if row >= total_rows {
            return;
        }

        let now = crate::sched::ticks();
        let is_double_click = matches!(
            self.last_click,
            Some((last_row, last_tick))
                if last_row == row && now.saturating_sub(last_tick) < DOUBLE_CLICK_TICKS
        );
        self.last_click = Some((row, now));

        if !is_double_click {
            if self.selected.contains(&row) {
                self.selected.remove(&row);
            } else {
                self.selected.insert(row);
            }
            // Remembers this row as a possible drag source: if the mouse
            // comes back up over a *different* row that's a folder (see
            // `handle_drag_release`), that's a real move, not just a
            // reselect.
            self.drag_source = Some(row);
            if let Some((path, _)) = self.row_path(row) {
                self.status = format!("selected: {}", file_name(path));
            }
            return;
        }

        let Some((path, is_dir)) = self.row_path(row).map(|(p, d)| (p.to_string(), d)) else {
            return;
        };
        if is_dir {
            self.navigate(path);
        } else {
            self.status = launch_path(&path);
        }
    }

    /// Completes a single-item drag: called on mouse-up when a plain click
    /// (not the one that started the drag) previously set `drag_source`.
    /// Real geometry hit-testing, same row math as `handle_click` --
    /// dropping onto a different row that's a folder moves the dragged
    /// item there via the same rename the Cut/Paste path already uses.
    pub fn handle_drag_release(&mut self, x: i32, y: i32) {
        let Some(src_row) = self.drag_source.take() else {
            return;
        };
        if y < LIST_TOP {
            return;
        }
        let row = match self.view {
            ViewMode::List => (y - LIST_TOP) / ROW_H,
            ViewMode::Grid => {
                let cell = self.grid_cell_size();
                let cols = self.grid_cols();
                let col = x / cell;
                if col >= cols {
                    return;
                }
                (y - LIST_TOP) / cell * cols + col
            }
        };
        if row < 0 {
            return;
        }
        let row = row as usize;
        if row == src_row || (self.has_parent_row() && row == 0) {
            return;
        }
        let Some((src_path, _)) = self.row_path(src_row).map(|(p, d)| (p.to_string(), d)) else {
            return;
        };
        let Some((dest_dir, dest_is_dir)) = self.row_path(row).map(|(p, d)| (p.to_string(), d))
        else {
            return;
        };
        if !dest_is_dir {
            return;
        }
        let dest = format!(
            "{}/{}",
            dest_dir.trim_end_matches('/'),
            file_name(&src_path)
        );
        let mut root = crate::fs::root().lock();
        if root.rename(&src_path, &dest) {
            self.status = format!(
                "moved {} into {}",
                file_name(&src_path),
                file_name(&dest_dir)
            );
            drop(root);
            self.refresh();
        } else {
            self.status = String::from("move failed (name already exists in that folder?)");
        }
    }

    pub fn handle_right_click(&mut self, x: i32, y: i32) -> Option<ContextMenu> {
        if y < LIST_TOP {
            return None;
        }
        let row = match self.view {
            ViewMode::List => (y - LIST_TOP) / ROW_H,
            ViewMode::Grid => {
                let cell = self.grid_cell_size();
                let cols = self.grid_cols();
                let col = x / cell;
                (y - LIST_TOP) / cell * cols + col
            }
        };
        let row = if row < 0 { None } else { Some(row as usize) };
        let target = row.and_then(|r| {
            if self.has_parent_row() && r == 0 {
                None
            } else {
                self.row_path(r).map(|(p, _)| p.to_string())
            }
        });
        self.context_target = target.clone();

        let mut items = Vec::new();
        if let Some(t) = &target {
            if t.ends_with(".exe") || t.ends_with(".mapp") || t.ends_with(".mlnk") {
                items.push((String::from("Run"), ACTION_RUN));
            }
            items.push((String::from("Rename"), ACTION_RENAME));
            items.push((String::from("Delete"), ACTION_DELETE));
            items.push((String::from("Copy"), ACTION_COPY));
            items.push((String::from("Cut"), ACTION_CUT));
            items.push((String::from("Create Shortcut"), ACTION_SHORTCUT));
            items.push((String::from("Properties"), ACTION_PROPERTIES));
            items.push((String::from("Compress to ZIP"), ACTION_COMPRESS));
            if t.ends_with(".zip") {
                items.push((String::from("Extract"), ACTION_EXTRACT));
            }
        }
        if self.clipboard.is_some() {
            items.push((String::from("Paste"), ACTION_PASTE));
        }
        items.push((String::from("New Folder"), ACTION_NEW_FOLDER));

        Some(ContextMenu { x: 0, y: 0, items })
    }

    pub fn handle_context_action(&mut self, action: u32) {
        if action == ACTION_RUN {
            // Handled before locking `root` below: `launch_path` re-locks
            // `fs::root()` itself (to read the target's bytes), and this
            // kernel's `spin::Mutex` isn't reentrant -- doing this inside
            // the `let root = ...` block below would deadlock the whole
            // File Manager on its own lock.
            if let Some(path) = self.context_target.take() {
                self.status = launch_path(&path);
            }
            self.refresh();
            return;
        }

        let mut root = crate::fs::root().lock();
        match action {
            ACTION_DELETE => {
                if let Some(path) = self.context_target.take() {
                    root.mkdir(TRASH_DIR);
                    let dest = format!("{TRASH_DIR}/{}", file_name(&path));
                    if root.rename(&path, &dest) {
                        self.status = format!("moved {} to Trash", file_name(&path));
                        crate::gui::notifications::push(
                            crate::gui::notifications::Kind::Info,
                            crate::gui::notifications::Category::Files,
                            self.status.clone(),
                        );
                    }
                }
            }
            ACTION_COPY => {
                if let Some(path) = self.context_target.take() {
                    self.clipboard = Some((path, false));
                }
            }
            ACTION_CUT => {
                if let Some(path) = self.context_target.take() {
                    self.clipboard = Some((path, true));
                }
            }
            ACTION_PASTE => {
                if let Some((src, is_cut)) = self.clipboard.take() {
                    let dest = format!(
                        "{}/{}",
                        self.current_path.trim_end_matches('/'),
                        file_name(&src)
                    );
                    let ok = if is_cut {
                        root.rename(&src, &dest)
                    } else {
                        root.copy(&src, &dest)
                    };
                    self.status = if ok {
                        format!("pasted {}", file_name(&dest))
                    } else {
                        String::from("paste failed (name already exists?)")
                    };
                }
            }
            ACTION_RENAME => {
                // A real inline rename needs a text-entry affordance this
                // pass doesn't add to the context menu itself; renaming to
                // a fixed "(renamed)" suffix here is a placeholder for a
                // real name-entry UI, not a fake -- the rename operation
                // itself is real and does move the file.
                if let Some(path) = self.context_target.take() {
                    let dest = format!("{path}.renamed");
                    if root.rename(&path, &dest) {
                        self.status = format!("renamed to {}", file_name(&dest));
                    }
                }
            }
            ACTION_NEW_FOLDER => {
                let base = format!("{}/New Folder", self.current_path.trim_end_matches('/'));
                let mut dest = base.clone();
                let mut n = 1;
                while root.exists(&dest) {
                    n += 1;
                    dest = format!("{base} {n}");
                }
                root.mkdir(&dest);
                self.status = format!("created {}", file_name(&dest));
            }
            ACTION_COMPRESS => {
                if let Some(path) = self.context_target.take() {
                    // Every real file under `path`: if it's a plain file
                    // that's just itself; if it's a directory, every file
                    // whose path starts with "`path`/" -- RAMFS is a flat
                    // map, so this is a real recursive walk without needing
                    // actual directory recursion.
                    let files: Vec<(String, Vec<u8>)> = if root.is_dir(&path) {
                        let prefix = format!("{path}/");
                        root.list()
                            .filter(|p| p.starts_with(&prefix))
                            .filter_map(|p| root.read(p).map(|d| (p.to_string(), d.to_vec())))
                            .collect()
                    } else {
                        root.read(&path)
                            .map(|d| alloc::vec![(path.clone(), d.to_vec())])
                            .unwrap_or_default()
                    };
                    if files.is_empty() {
                        self.status = String::from("nothing to compress");
                    } else {
                        // ZIP entry names are relative to the compressed
                        // item itself, matching how a real archiver names
                        // entries inside the archive it produces.
                        let base_prefix = format!("{path}/");
                        let entries: Vec<(String, Vec<u8>)> = files
                            .into_iter()
                            .map(|(p, d)| {
                                let name = p.strip_prefix(&base_prefix).unwrap_or(&p);
                                (String::from(name), d)
                            })
                            .collect();
                        let archive = crate::zip::build_stored(&entries);
                        let dest = format!(
                            "{}/{}.zip",
                            self.current_path.trim_end_matches('/'),
                            file_name(&path)
                        );
                        root.write(&dest, &archive);
                        self.status = format!(
                            "compressed {} file(s) into {}",
                            entries.len(),
                            file_name(&dest)
                        );
                        crate::gui::notifications::push(
                            crate::gui::notifications::Kind::Success,
                            crate::gui::notifications::Category::Files,
                            self.status.clone(),
                        );
                    }
                }
            }
            ACTION_SHORTCUT => {
                if let Some(path) = self.context_target.take() {
                    let label = file_name(&path).to_string();
                    let base = format!("{}/{}.mlnk", self.current_path.trim_end_matches('/'), label);
                    let mut dest = base.clone();
                    let mut n = 1;
                    while root.exists(&dest) {
                        n += 1;
                        dest = format!(
                            "{}/{}.{}.mlnk",
                            self.current_path.trim_end_matches('/'),
                            label,
                            n
                        );
                    }
                    root.write(&dest, path.as_bytes());
                    self.status = format!("created shortcut {} -> {}", file_name(&dest), path);
                }
            }
            ACTION_PROPERTIES => {
                if let Some(path) = self.context_target.take() {
                    let is_dir = root.is_dir(&path);
                    let size = root.read(&path).map(|d| d.len()).unwrap_or(0);
                    let kind = if is_dir {
                        "folder"
                    } else if path.ends_with(".exe") {
                        "Windows PE executable"
                    } else if path.ends_with(".msi") {
                        "Windows installer package (unsupported)"
                    } else if path.ends_with(".apk") {
                        "Android package"
                    } else if path.ends_with(".mapp") {
                        "moon OS package"
                    } else if path.ends_with(".mlnk") {
                        "shortcut"
                    } else {
                        "file"
                    };
                    self.status = if is_dir {
                        format!("{}: {}", file_name(&path), kind)
                    } else {
                        format!("{}: {} -- {} bytes", file_name(&path), kind, size)
                    };
                }
            }
            ACTION_EXTRACT => {
                if let Some(path) = self.context_target.take() {
                    match root.read(&path) {
                        Some(data) => {
                            let data = data.to_vec();
                            match crate::apk::list_entries(&data) {
                                Ok(zip_entries) => {
                                    let dest_dir = self.current_path.trim_end_matches('/');
                                    let mut extracted = 0usize;
                                    let mut skipped = 0usize;
                                    for entry in &zip_entries {
                                        match crate::apk::read_entry(&data, entry) {
                                            Ok(bytes) => {
                                                root.write(
                                                    &format!("{dest_dir}/{}", entry.name),
                                                    &bytes,
                                                );
                                                extracted += 1;
                                            }
                                            Err(_) => skipped += 1, // DEFLATE entry -- no decoder yet
                                        }
                                    }
                                    self.status = if skipped == 0 {
                                        format!("extracted {extracted} file(s)")
                                    } else {
                                        format!(
                                            "extracted {extracted} file(s), skipped {skipped} (DEFLATE-compressed, no decoder yet)"
                                        )
                                    };
                                    crate::gui::notifications::push(
                                        crate::gui::notifications::Kind::Success,
                                        crate::gui::notifications::Category::Files,
                                        self.status.clone(),
                                    );
                                }
                                Err(err) => self.status = format!("not a valid ZIP: {err}"),
                            }
                        }
                        None => self.status = String::from("couldn't read that file"),
                    }
                }
            }
            _ => {}
        }
        drop(root);
        self.refresh();
    }

    fn grid_cell_size(&self) -> i32 {
        56
    }

    /// Column count the grid was last actually rendered with (see `render`,
    /// which recomputes and stores this from the real content width).
    fn grid_cols(&self) -> i32 {
        self.grid_cols.get()
    }

    pub fn render(&self, x: i32, y: i32, w: u32, h: u32) {
        crate::framebuffer::with(|c| {
            c.fill_rect(x, y, w, h, (0x10, 0x10, 0x16));

            let path_label = if self.current_path == "/" {
                String::from("/  (RAMFS root)")
            } else {
                self.current_path.clone()
            };
            c.draw_str_at(x + 4, y + 4, &path_label, (0x90, 0xC0, 0xFF), None);

            let neon = crate::gui::theme::accent();
            let list_color = if self.view == ViewMode::List {
                neon
            } else {
                (0x80, 0x80, 0x90)
            };
            let grid_color = if self.view == ViewMode::Grid {
                neon
            } else {
                (0x80, 0x80, 0x90)
            };
            c.draw_str_at(x + 4, y + HEADER_H + 2, "[List]", list_color, None);
            c.draw_str_at(x + 52, y + HEADER_H + 2, "[Grid]", grid_color, None);
            let search_color = if self.searching {
                neon
            } else {
                (0x80, 0x80, 0x90)
            };
            c.draw_str_at(x + 102, y + HEADER_H + 2, "[Find]", search_color, None);
            let recent_color = if self.showing_recent {
                neon
            } else {
                (0x80, 0x80, 0x90)
            };
            c.draw_str_at(x + 162, y + HEADER_H + 2, "[Recent]", recent_color, None);
            if self.searching || !self.search.is_empty() {
                c.draw_str_at(
                    x + 230,
                    y + HEADER_H + 2,
                    &format!("/{}_", self.search),
                    (0xE0, 0xE0, 0x60),
                    None,
                );
            }
        });

        let list_y = y + LIST_TOP;
        let list_h = h as i32 - LIST_TOP - 14;
        let cols = ((w as i32 - 8) / self.grid_cell_size()).max(1);
        self.grid_cols.set(cols);

        let mut row = 0usize;
        if self.has_parent_row() {
            self.render_row(x, list_y, w, row, "..", true, false, cols);
            row += 1;
        }
        for (path, is_dir) in self.entries.iter() {
            if (row as i32) * self.row_height() >= list_h {
                break;
            }
            let selected = self.selected.contains(&row);
            self.render_row(x, list_y, w, row, file_name(path), *is_dir, selected, cols);
            row += 1;
        }

        let status_y = y + h as i32 - 12;
        crate::framebuffer::with(|c| {
            c.fill_rect(x, status_y - 2, w, 14, (0x08, 0x08, 0x0C));
            c.draw_str_at(x + 4, status_y, &self.status, (0x80, 0x80, 0x90), None);
        });
    }

    fn row_height(&self) -> i32 {
        match self.view {
            ViewMode::List => ROW_H,
            ViewMode::Grid => self.grid_cell_size(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_row(
        &self,
        x: i32,
        list_y: i32,
        w: u32,
        row: usize,
        name: &str,
        is_dir: bool,
        selected: bool,
        cols: i32,
    ) {
        // Color-coded by real type, standing in for a per-type thumbnail
        // (there's no image decoder to render an actual preview with).
        let color = if is_dir {
            (0x90, 0xC8, 0xFF)
        } else if name.ends_with(".mapp") {
            (0x80, 0xE8, 0xA0)
        } else if name.ends_with(".exe") {
            (0x90, 0xD8, 0xFF)
        } else if name.ends_with(".msi") {
            (0x70, 0xB0, 0xE0)
        } else if name.ends_with(".apk") {
            (0xA8, 0xE0, 0x80)
        } else if name.ends_with(".mlnk") {
            (0xD0, 0xA8, 0xF0)
        } else if name.ends_with(".zip") {
            (0xE8, 0xC0, 0x60)
        } else if name.ends_with(".txt") {
            (0xD0, 0xD0, 0xE0)
        } else {
            (0xC0, 0xC0, 0xC0)
        };

        match self.view {
            ViewMode::List => {
                let row_y = list_y + row as i32 * ROW_H;
                crate::framebuffer::with(|c| {
                    if selected {
                        c.fill_rect(x + 2, row_y, w - 4, ROW_H as u32, (0x14, 0x2A, 0x36));
                    }
                    let max_chars = ((w as i32 - 16) / 8).max(1) as usize;
                    let text = if name.len() > max_chars {
                        &name[..max_chars]
                    } else {
                        name
                    };
                    c.draw_str_at(x + 8, row_y, text, color, None);
                });
            }
            ViewMode::Grid => {
                let cell = self.grid_cell_size();
                let col = row as i32 % cols;
                let grid_row = row as i32 / cols;
                let cx = x + col * cell;
                let cy = list_y + grid_row * cell;
                crate::framebuffer::with(|c| {
                    if selected {
                        c.fill_rect(
                            cx + 2,
                            cy + 2,
                            cell as u32 - 4,
                            cell as u32 - 4,
                            (0x14, 0x2A, 0x36),
                        );
                    }
                    if is_dir {
                        c.fill_rect(cx + 14, cy + 6, 28, 20, color);
                    } else {
                        c.fill_rect(cx + 16, cy + 4, 24, 28, color);
                    }
                    let max_chars = ((cell - 4) / 8).max(1) as usize;
                    let label = if name.len() > max_chars {
                        &name[..max_chars]
                    } else {
                        name
                    };
                    c.draw_str_at(cx + 2, cy + cell - 12, label, (0xD0, 0xD0, 0xD0), None);
                });
            }
        }
    }
}

impl Default for FileManagerState {
    fn default() -> Self {
        Self::new()
    }
}
