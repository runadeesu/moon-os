//! Moon Store: a local catalog of installed `.mapp` packages *and*
//! installed Android packages (`crate::androidpkg`), with search, a
//! category filter, a real description/category per entry, and
//! Launch/Uninstall actions that actually run (or, for Android packages,
//! honestly refuse to run) or remove the package.
//!
//! Honest limitations, unchanged from before: there's no networked package
//! repository, so "the store" and "what's installed" are still the same
//! list -- installing something new means dropping a `.mapp`/`.exe`/`.apk`
//! into RAMFS via the File Manager (which is exactly how the bundled
//! packages and the Android metadata registry get populated). That also
//! means there's deliberately no download-progress bar, star ratings, or
//! review counts -- see `net::http`/the Browser app for where real,
//! over-the-wire progress would actually belong once package downloads
//! exist, and there are no real users submitting reviews to fake numbers
//! for. Screenshots/icons are text-only since there's no real image asset
//! pipeline. Android entries are listed and launchable-in-theory the same
//! as `.mapp` ones, but "Launch" on one always reports the truth: no
//! Dalvik/ART interpreter, no Android framework, so its code cannot
//! actually run here -- see `androidpkg.rs`'s doc comment.

use crate::framebuffer;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::Cell;

const ROW_H: i32 = 34;
const HEADER_H: i32 = 20;
const TOOLBAR_H: i32 = 16;
const LIST_TOP: i32 = HEADER_H + TOOLBAR_H;
/// Width (in px, from the row's right edge) of the `[Uninstall]` label --
/// a click inside this strip uninstalls instead of launching. Matches the
/// column `render` actually draws it in ("[Uninstall]" is 11 chars * 8px,
/// plus a little padding).
const UNINSTALL_ZONE_W: i32 = 11 * 8 + 12;

/// Real, hand-written metadata for the packages this build actually bundles
/// -- not placeholder lorem ipsum, an honest one-line description of what
/// each one really does.
fn describe(name: &str) -> (&'static str, &'static str) {
    match name {
        "init" => (
            "Utility",
            "First ring-3 process spawned at boot; proves usermode + syscalls work.",
        ),
        "counter" => (
            "Demo",
            "Counts 0..4 in a second ring-3 process, proving multi-process spawn.",
        ),
        _ => ("App", "No description available."),
    }
}

/// One catalog row: either a real `.mapp` package or a real (metadata-only)
/// installed Android package. Unifies the two for listing/search/category
/// filtering/sorting without pretending they're the same kind of thing --
/// `description()`/`category()` are honest about which is which.
enum Entry {
    Mapp(crate::pkg::InstalledPackage),
    Android(crate::androidpkg::InstalledApk),
}

impl Entry {
    fn title(&self) -> String {
        match self {
            Entry::Mapp(p) => format!("{} v{}", p.name, p.version),
            Entry::Android(a) => format!("{} (Android)", a.label),
        }
    }

    fn category(&self) -> &'static str {
        match self {
            Entry::Mapp(p) => describe(&p.name).0,
            Entry::Android(_) => "Android",
        }
    }

    fn description(&self) -> String {
        match self {
            Entry::Mapp(p) => describe(&p.name).1.to_string(),
            Entry::Android(a) => format!(
                "package {} -- metadata only; no Android runtime to actually run its code",
                a.package
            ),
        }
    }

    fn sort_key(&self) -> String {
        match self {
            Entry::Mapp(p) => p.name.clone(),
            Entry::Android(a) => a.package.clone(),
        }
    }

    fn search_text(&self) -> String {
        match self {
            Entry::Mapp(p) => p.name.clone(),
            Entry::Android(a) => format!("{} {}", a.package, a.label),
        }
    }
}

pub struct StoreState {
    status: String,
    searching: bool,
    search: String,
    /// Index into `categories()`; 0 always means "All". A real, working
    /// filter over the actual categories present -- not a fixed dropdown of
    /// categories that might not even have packages in them.
    category_index: usize,
    /// Content-area width from the most recent `render()` call -- needed by
    /// `handle_click` to tell the `[Launch]`/`[Uninstall]` columns apart,
    /// since both are laid out relative to the right edge. Interior
    /// mutability for the same reason as `FileManagerState::grid_cols`:
    /// `render` only gets `&self`.
    content_w: Cell<u32>,
}

impl StoreState {
    pub fn new() -> Self {
        Self {
            status: String::from("click a package to launch it"),
            searching: false,
            search: String::new(),
            category_index: 0,
            content_w: Cell::new(0),
        }
    }

    fn all_entries(&self) -> Vec<Entry> {
        let mut entries: Vec<Entry> = crate::pkg::installed().into_iter().map(Entry::Mapp).collect();
        entries.extend(crate::androidpkg::installed().into_iter().map(Entry::Android));
        entries
    }

    /// `["All", ...every distinct category actually present, sorted]` --
    /// computed from the real installed set each time, so it never lists a
    /// category with nothing in it.
    fn categories(&self) -> Vec<&'static str> {
        let mut cats: Vec<&'static str> = self.all_entries().iter().map(Entry::category).collect();
        cats.sort_unstable();
        cats.dedup();
        let mut out = alloc::vec!["All"];
        out.extend(cats);
        out
    }

    fn current_category(&self) -> &'static str {
        let cats = self.categories();
        cats[self.category_index % cats.len()]
    }

    fn filtered(&self) -> Vec<Entry> {
        let mut entries = self.all_entries();
        if !self.search.is_empty() {
            let needle = self.search.to_ascii_lowercase();
            entries.retain(|e| e.search_text().to_ascii_lowercase().contains(&needle));
        }
        let category = self.current_category();
        if category != "All" {
            entries.retain(|e| e.category() == category);
        }
        // Real, deterministic ordering (category, then name) -- not a
        // popularity/ranking sort, since there's no real usage data to rank
        // by.
        entries.sort_by(|a, b| a.category().cmp(b.category()).then_with(|| a.sort_key().cmp(&b.sort_key())));
        entries
    }

    pub fn handle_char(&mut self, ch: u8) {
        if !self.searching {
            return;
        }
        match ch {
            b'\n' => self.searching = false,
            0x08 => {
                self.search.pop();
            }
            0x20..=0x7E => self.search.push(ch as char),
            _ => {}
        }
    }

    /// `x`/`y` are local to the widget's content area, same convention as
    /// `Window::handle_click`.
    pub fn handle_click(&mut self, x: i32, y: i32) {
        if y < HEADER_H {
            return;
        }
        if y < LIST_TOP {
            if x < 60 {
                self.searching = !self.searching;
                if !self.searching {
                    self.search.clear();
                }
            } else {
                let cats = self.categories();
                self.category_index = (self.category_index + 1) % cats.len();
            }
            return;
        }

        let row = (y - LIST_TOP) / ROW_H;
        if row < 0 {
            return;
        }
        let entries = self.filtered();
        let Some(entry) = entries.get(row as usize) else {
            return;
        };

        let content_w = self.content_w.get() as i32;
        if content_w > 0 && x >= content_w - UNINSTALL_ZONE_W {
            let (removed, label) = match entry {
                Entry::Mapp(p) => (crate::fs::root().lock().remove(&p.file_name), p.name.clone()),
                Entry::Android(a) => (crate::androidpkg::uninstall(&a.package), a.package.clone()),
            };
            if removed {
                self.status = format!("uninstalled {label}");
                crate::gui::notifications::push(
                    crate::gui::notifications::Kind::Info,
                    crate::gui::notifications::Category::Packages,
                    self.status.clone(),
                );
            }
            return;
        }

        self.status = match entry {
            Entry::Mapp(p) => match crate::pkg::run(&p.file_name) {
                Ok(name) => format!("launched {name}"),
                Err(err) => format!("launch failed: {err}"),
            },
            Entry::Android(a) => match crate::androidpkg::launch(&a.package) {
                Ok(()) => String::from("launched"),
                Err(err) => err,
            },
        };
    }

    pub fn render(&self, x: i32, y: i32, w: u32, h: u32) {
        self.content_w.set(w);
        let entries = self.filtered();
        let neon = crate::gui::theme::accent();

        framebuffer::with(|c| {
            c.fill_rect(x, y, w, h, (0x10, 0x10, 0x16));
            c.draw_str_at(
                x + 4,
                y + 4,
                "Moon Store -- local catalog",
                (0x90, 0xC0, 0xFF),
                None,
            );

            let search_color = if self.searching {
                neon
            } else {
                (0x80, 0x80, 0x90)
            };
            c.draw_str_at(x + 4, y + HEADER_H + 2, "[Find]", search_color, None);
            if self.searching || !self.search.is_empty() {
                c.draw_str_at(
                    x + 60,
                    y + HEADER_H + 2,
                    &format!("/{}_", self.search),
                    (0xE0, 0xE0, 0x60),
                    None,
                );
            }
            c.draw_str_at(
                x + w as i32 - 120,
                y + HEADER_H + 2,
                &format!("Category: {}", self.current_category()),
                neon,
                None,
            );

            for (row, entry) in entries.iter().enumerate() {
                let row_y = y + LIST_TOP + row as i32 * ROW_H;
                if row_y + ROW_H > y + h as i32 {
                    break;
                }
                c.fill_rect(x + 4, row_y, w - 8, ROW_H as u32 - 4, (0x18, 0x1C, 0x26));
                c.draw_str_at(
                    x + 8,
                    row_y + 4,
                    &format!("{}  [{}]", entry.title(), entry.category()),
                    (0xE0, 0xE0, 0xE0),
                    None,
                );
                let desc = entry.description();
                let max_desc_chars = ((w as i32 - 16) / 8).max(1) as usize;
                let desc_line = if desc.len() > max_desc_chars {
                    &desc[..max_desc_chars]
                } else {
                    &desc
                };
                c.draw_str_at(x + 8, row_y + 16, desc_line, (0x90, 0x90, 0xA0), None);
                c.draw_str_at(
                    x + w as i32 - UNINSTALL_ZONE_W - 8 * 8 - 8,
                    row_y + 4,
                    "[Launch]",
                    (0x80, 0xE8, 0xA0),
                    None,
                );
                c.draw_str_at(
                    x + w as i32 - UNINSTALL_ZONE_W + 4,
                    row_y + 4,
                    "[Uninstall]",
                    (0xE8, 0x90, 0x90),
                    None,
                );
            }
        });

        let status_y = y + h as i32 - 12;
        framebuffer::with(|c| {
            c.fill_rect(x, status_y - 2, w, 14, (0x08, 0x08, 0x0C));
            c.draw_str_at(x + 4, status_y, &self.status, (0x80, 0x80, 0x90), None);
        });
    }
}

impl Default for StoreState {
    fn default() -> Self {
        Self::new()
    }
}
