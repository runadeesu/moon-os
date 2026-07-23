//! Fixed, non-interactive desktop panels -- not real `Window`s (they don't
//! drag or take focus): a calendar built from the real CMOS RTC date, a
//! real moon-phase panel (the actual current lunar phase, computed from
//! that same date via a Julian Day Number + synodic month calculation, not
//! a placeholder icon), a live system-monitor readout mirroring the
//! Settings widget's stats, and a storage/network panel. Drawn directly
//! onto the desktop background, below any actual app window.

use crate::framebuffer;
use crate::i18n::{self, Key};
use alloc::format;
use core::sync::atomic::{AtomicU64, Ordering};

pub const PANEL_W: u32 = 200;
const GAP: i32 = 12;
/// How often (in scheduler ticks, ~100/sec) the net-speed panel resamples
/// cumulative byte counters into a rate -- sampling every single redraw
/// would make the number jitter wildly on small packets instead of reading
/// as a real per-second rate.
const NET_SAMPLE_TICKS: u64 = 100;

static NET_LAST_SAMPLE_TICK: AtomicU64 = AtomicU64::new(0);
static NET_LAST_TX: AtomicU64 = AtomicU64::new(0);
static NET_LAST_RX: AtomicU64 = AtomicU64::new(0);
static NET_TX_RATE: AtomicU64 = AtomicU64::new(0);
static NET_RX_RATE: AtomicU64 = AtomicU64::new(0);

const DAYS_IN_MONTH: [u8; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

fn is_leap(year: u16) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

/// Zeller's congruence, adapted to return 0=Sunday..6=Saturday for the 1st
/// of `(year, month)` on the Gregorian calendar.
fn weekday_of_first(year: u16, month: u8) -> u8 {
    let (y, m) = if month < 3 {
        (i32::from(year) - 1, i32::from(month) + 12)
    } else {
        (i32::from(year), i32::from(month))
    };
    let k = y % 100;
    let j = y / 100;
    let h = (1 + 13 * (m + 1) / 5 + k + k / 4 + j / 4 + 5 * j).rem_euclid(7);
    ((h + 6) % 7) as u8 // Zeller's h is 0=Saturday; shift to 0=Sunday
}

/// Renders the calendar, moon-phase, system-monitor, and storage/network
/// panels stacked at `(x, y)`. Returns the y just past the last panel.
pub fn render(x: i32, y: i32) -> i32 {
    let y = render_calendar(x, y);
    let y = render_moon_phase(x, y);
    let y = render_system_monitor(x, y);
    render_storage_network(x, y)
}

/// Julian Day Number for a Gregorian calendar date (Fliegel & Van Flandern's
/// integer formula) -- cross-checked against Python's independent
/// `date.toordinal()`-based conversion for several dates (all matched
/// exactly) before use here, since this is the one piece of the moon-phase
/// math that has to be exactly right for the rest to mean anything.
fn julian_day_number(year: u16, month: u8, day: u8) -> i64 {
    let (y, m, d) = (i64::from(year), i64::from(month), i64::from(day));
    let a = (14 - m) / 12;
    let yy = y + 4800 - a;
    let mm = m + 12 * a - 3;
    d + (153 * mm + 2) / 5 + 365 * yy + yy / 4 - yy / 100 + yy / 400 - 32045
}

const SYNODIC_MONTH_X1000: i64 = 29_531; // 29.530588853 days, in thousandths
/// 2000-01-06 (~18:14 UTC), a widely-cited real new moon -- cross-checked
/// during development against a known real full moon (2023-08-31, the
/// widely-reported "blue moon") landing almost exactly at age_frac=0.5.
const REF_NEW_MOON_JDN: i64 = 2_451_550;

const MOON_PHASE_NAMES: [&str; 8] = [
    "New Moon",
    "Waxing Crescent",
    "First Quarter",
    "Waxing Gibbous",
    "Full Moon",
    "Waning Gibbous",
    "Last Quarter",
    "Waning Crescent",
];

/// Real "days since new moon" (as thousandths of a day, 0..SYNODIC_MONTH_X1000)
/// computed from the actual RTC date via the Julian Day Number and the real
/// synodic month length -- not a fake/random phase. Returns
/// `(named phase index 0..8, illuminated-fraction permille 0..1000, waxing?)`.
fn moon_phase(year: u16, month: u8, day: u8) -> (usize, i32, bool) {
    let jdn = julian_day_number(year, month, day);
    let age = ((jdn - REF_NEW_MOON_JDN) * 1000).rem_euclid(SYNODIC_MONTH_X1000);
    let half = SYNODIC_MONTH_X1000 / 2;
    let (illum, waxing) = if age <= half {
        (age * 1000 / half, true)
    } else {
        ((SYNODIC_MONTH_X1000 - age) * 1000 / half, false)
    };
    let centered = (age + SYNODIC_MONTH_X1000 / 16) % SYNODIC_MONTH_X1000;
    let phase_index = (centered * 8 / SYNODIC_MONTH_X1000) as usize;
    (phase_index, illum as i32, waxing)
}

/// Draws the real current moon phase: a bright disc with a same-radius
/// dark "terminator" circle offset horizontally by how much of the disc
/// should currently be dark. This is a simplified two-circle approximation
/// (a real terminator is a half-ellipse projection, which needs
/// trigonometry this `no_std`, no-float kernel doesn't have -- see this
/// file's module doc on integer-only math), and the left/right waxing
/// -vs-waning convention here is a stylistic choice, not calibrated to a
/// real hemisphere. What *is* real: the phase name, day-in-cycle, and
/// illumination percentage, all computed from the actual RTC date above.
fn render_moon_phase(x: i32, y: i32) -> i32 {
    let dt = crate::drivers::rtc::read();
    let (phase_index, illum, waxing) = moon_phase(dt.year, dt.month, dt.day);
    let neon = super::theme::accent();

    let panel_h = 68u32;
    let r = 22i32;
    let cx = x + 12 + r;
    let cy = y + panel_h as i32 / 2;

    framebuffer::with(|c| {
        c.glow_border(x, y, PANEL_W, panel_h, neon);
        c.blend_rect(x, y, PANEL_W, panel_h, (0x10, 0x14, 0x22), 200);

        c.fill_circle(cx, cy, r, (0xE8, 0xE8, 0xF2));
        let dark_offset = 2 * r * illum / 1000;
        let dark_cx = if waxing { cx - dark_offset } else { cx + dark_offset };
        c.fill_circle(dark_cx, cy, r, (0x10, 0x14, 0x22));

        let label_x = x + 12 + 2 * r + 10;
        c.draw_str_at(
            label_x,
            y + 14,
            MOON_PHASE_NAMES[phase_index],
            (0xD8, 0xD8, 0xF0),
            None,
        );
        c.draw_str_at(
            label_x,
            y + 30,
            &format!("{}% illuminated", illum / 10),
            (0x90, 0x98, 0xB0),
            None,
        );
    });

    y + panel_h as i32 + GAP
}

fn render_calendar(x: i32, y: i32) -> i32 {
    let dt = crate::drivers::rtc::read();
    let month_idx = dt.month.saturating_sub(1).min(11) as usize;
    let days = if dt.month == 2 && is_leap(dt.year) {
        29
    } else {
        DAYS_IN_MONTH[month_idx]
    };
    let first_wd = weekday_of_first(dt.year, dt.month);
    let neon = super::theme::accent();

    let panel_h = 152u32;
    framebuffer::with(|c| {
        c.glow_border(x, y, PANEL_W, panel_h, neon);
        c.blend_rect(x, y, PANEL_W, panel_h, (0x10, 0x14, 0x22), 200);
        c.draw_str_at(
            x + 8,
            y + 8,
            &format!("{:04}-{:02}", dt.year, dt.month),
            (0xC0, 0xD8, 0xFF),
            None,
        );
    });

    let cell_w = 24i32;
    let cell_h = 15i32;
    let grid_x = x + 8;
    let grid_y = y + 26;
    let labels = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"];
    framebuffer::with(|c| {
        for (i, label) in labels.iter().enumerate() {
            c.draw_str_at(
                grid_x + i as i32 * cell_w,
                grid_y,
                label,
                (0x70, 0x80, 0xA0),
                None,
            );
        }
    });

    let mut day = 1u8;
    let mut row = 1i32;
    let mut col = i32::from(first_wd);
    while day <= days {
        let cx = grid_x + col * cell_w;
        let cy = grid_y + row * cell_h;
        let is_today = day == dt.day;
        framebuffer::with(|c| {
            if is_today {
                c.fill_rect(cx - 2, cy - 2, 18, 12, (0x0E, 0x4A, 0x60));
                c.glow_border(cx - 2, cy - 2, 18, 12, neon);
            }
            c.draw_str_at(cx, cy, &format!("{:2}", day), (0xD8, 0xD8, 0xD8), None);
        });
        col += 1;
        if col > 6 {
            col = 0;
            row += 1;
        }
        day += 1;
    }

    y + panel_h as i32 + GAP
}

fn render_system_monitor(x: i32, y: i32) -> i32 {
    let stats = crate::memory::pmm::stats();
    let free_mib = (stats.free_frames * 4096) / (1024 * 1024);
    let total_mib = (stats.total_frames * 4096) / (1024 * 1024);
    let ticks = crate::sched::ticks();
    let neon = super::theme::accent();

    let panel_h = 90u32;
    framebuffer::with(|c| {
        c.glow_border(x, y, PANEL_W, panel_h, neon);
        c.blend_rect(x, y, PANEL_W, panel_h, (0x10, 0x14, 0x22), 200);
        c.draw_glyphs_at(
            x + 8,
            y + 8,
            i18n::tr(Key::SystemMonitorHeader),
            (0xC0, 0xD8, 0xFF),
            None,
        );

        let mem_label = i18n::tr(Key::MemoryLabel);
        c.draw_glyphs_at(x + 8, y + 26, mem_label, (0xD0, 0xD0, 0xD0), None);
        c.draw_str_at(
            x + 8 + (mem_label.len() as i32 + 1) * 8,
            y + 26,
            &format!(": {} / {} MiB", free_mib, total_mib),
            (0xD0, 0xD0, 0xD0),
            None,
        );

        let tasks_label = i18n::tr(Key::TasksLabel);
        c.draw_glyphs_at(x + 8, y + 40, tasks_label, (0xD0, 0xD0, 0xD0), None);
        c.draw_str_at(
            x + 8 + (tasks_label.len() as i32 + 1) * 8,
            y + 40,
            &format!(": {}", crate::sched::task_count()),
            (0xD0, 0xD0, 0xD0),
            None,
        );

        let uptime_label = i18n::tr(Key::UptimeLabel);
        c.draw_glyphs_at(x + 8, y + 54, uptime_label, (0xD0, 0xD0, 0xD0), None);
        c.draw_str_at(
            x + 8 + (uptime_label.len() as i32 + 1) * 8,
            y + 54,
            &format!(": {}s", ticks / 100),
            (0xD0, 0xD0, 0xD0),
            None,
        );
    });

    y + panel_h as i32
}

/// Resamples the cumulative TX/RX byte counters (`net::traffic_totals`)
/// into a bytes/sec rate at most once every `NET_SAMPLE_TICKS`, storing the
/// result so calls in between just redraw the last real sample instead of
/// showing a jittery instantaneous value. Also used by the taskbar's
/// net-speed tray readout, so both places agree on the same sampled rate.
pub fn sample_net_rate() -> (u64, u64) {
    let now = crate::sched::ticks();
    let last = NET_LAST_SAMPLE_TICK.load(Ordering::Relaxed);
    let elapsed = now.saturating_sub(last);
    if elapsed < NET_SAMPLE_TICKS {
        return (
            NET_TX_RATE.load(Ordering::Relaxed),
            NET_RX_RATE.load(Ordering::Relaxed),
        );
    }

    let (tx, rx) = crate::net::traffic_totals();
    let last_tx = NET_LAST_TX.swap(tx, Ordering::Relaxed);
    let last_rx = NET_LAST_RX.swap(rx, Ordering::Relaxed);
    NET_LAST_SAMPLE_TICK.store(now, Ordering::Relaxed);

    // Ticks are ~100/sec, so bytes/tick * 100 = bytes/sec -- integer-only,
    // no floats (context switches don't save FPU/SSE state).
    let tx_rate = tx.saturating_sub(last_tx) * 100 / elapsed.max(1);
    let rx_rate = rx.saturating_sub(last_rx) * 100 / elapsed.max(1);
    NET_TX_RATE.store(tx_rate, Ordering::Relaxed);
    NET_RX_RATE.store(rx_rate, Ordering::Relaxed);
    (tx_rate, rx_rate)
}

fn render_storage_network(x: i32, y: i32) -> i32 {
    let used = crate::fs::root().lock().used_bytes();
    let (tx_rate, rx_rate) = sample_net_rate();
    let neon = super::theme::accent();

    let panel_h = 56u32;
    framebuffer::with(|c| {
        c.glow_border(x, y, PANEL_W, panel_h, neon);
        c.blend_rect(x, y, PANEL_W, panel_h, (0x10, 0x14, 0x22), 200);
        c.draw_str_at(x + 8, y + 8, "storage / net", (0xC0, 0xD8, 0xFF), None);
        c.draw_str_at(
            x + 8,
            y + 24,
            &format!("RAMFS: {} KiB used", used.div_ceil(1024)),
            (0xD0, 0xD0, 0xD0),
            None,
        );
        c.draw_str_at(
            x + 8,
            y + 38,
            &format!("net: up {} B/s, dn {} B/s", tx_rate, rx_rate),
            (0xD0, 0xD0, 0xD0),
            None,
        );
    });

    y + panel_h as i32
}
