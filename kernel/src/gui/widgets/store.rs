//! Moon Store: a local catalog of installed `.mapp` packages with search, a
//! category filter, a real description/category per package, and
//! Launch/Uninstall actions that actually run or remove the package.
//!
//! Honest limitations, unchanged from before: there's no networked package
//! repository, so "the store" and "what's installed" are still the same
//! list -- installing something new means dropping a `.mapp` into RAMFS
//! (currently only the two bundled packages exist). That also means there's
//! deliberately no download-progress bar: a real one would have nothing to
//! measure (installs are an instant RAMFS write), and a fake one would be
//! exactly the kind of dummy UI this project avoids -- see `net::http` /
//! the Browser app for where real, slow, over-the-wire progress would
//! actually belong once package downloads exist. Star ratings, review
//! counts, and a "featured/ranking" section are deliberately not here for
//! the same reason: with no real users submitting reviews, faking those
//! numbers would be dummy data. Screenshots/icons are text-only (name +
//! category) since there's no real image asset pipeline to draw from yet.

use crate::framebuffer;
use alloc::format;
use alloc::string::String;
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

    /// `["All", ...every distinct category actually present, sorted]` --
    /// computed from the real installed set each time, so it never lists a
    /// category with nothing in it.
    fn categories(&self) -> alloc::vec::Vec<&'static str> {
        let mut cats: alloc::vec::Vec<&'static str> = crate::pkg::installed()
            .iter()
            .map(|p| describe(&p.name).0)
            .collect();
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

    fn filtered(&self) -> alloc::vec::Vec<crate::pkg::InstalledPackage> {
        let mut packages = crate::pkg::installed();
        if !self.search.is_empty() {
            let needle = self.search.to_ascii_lowercase();
            packages.retain(|p| p.name.to_ascii_lowercase().contains(&needle));
        }
        let category = self.current_category();
        if category != "All" {
            packages.retain(|p| describe(&p.name).0 == category);
        }
        // Real, deterministic ordering (category, then name) -- not a
        // popularity/ranking sort, since there's no real usage data to rank
        // by.
        packages.sort_by(|a, b| {
            describe(&a.name)
                .0
                .cmp(describe(&b.name).0)
                .then_with(|| a.name.cmp(&b.name))
        });
        packages
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
        let packages = self.filtered();
        let Some(pkg) = packages.get(row as usize) else {
            return;
        };

        let content_w = self.content_w.get() as i32;
        if content_w > 0 && x >= content_w - UNINSTALL_ZONE_W {
            let removed = crate::fs::root().lock().remove(&pkg.file_name);
            if removed {
                self.status = format!("uninstalled {}", pkg.name);
                crate::gui::notifications::push(
                    crate::gui::notifications::Kind::Info,
                    crate::gui::notifications::Category::Packages,
                    self.status.clone(),
                );
            }
            return;
        }

        self.status = match crate::pkg::run(&pkg.file_name) {
            Ok(name) => format!("launched {}", name),
            Err(err) => format!("launch failed: {}", err),
        };
    }

    pub fn render(&self, x: i32, y: i32, w: u32, h: u32) {
        self.content_w.set(w);
        let packages = self.filtered();
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

            for (row, pkg) in packages.iter().enumerate() {
                let row_y = y + LIST_TOP + row as i32 * ROW_H;
                if row_y + ROW_H > y + h as i32 {
                    break;
                }
                let (category, desc) = describe(&pkg.name);
                c.fill_rect(x + 4, row_y, w - 8, ROW_H as u32 - 4, (0x18, 0x1C, 0x26));
                c.draw_str_at(
                    x + 8,
                    row_y + 4,
                    &format!("{} v{}  [{}]", pkg.name, pkg.version, category),
                    (0xE0, 0xE0, 0xE0),
                    None,
                );
                let max_desc_chars = ((w as i32 - 16) / 8).max(1) as usize;
                let desc_line = if desc.len() > max_desc_chars {
                    &desc[..max_desc_chars]
                } else {
                    desc
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
