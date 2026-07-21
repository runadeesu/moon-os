//! Simple scrolling text console drawn directly into the boot framebuffer
//! Limine hands us. No compositor, no windows yet -- just enough to prove
//! the kernel is alive with more than a serial log.

use crate::font::FONT8X8;
use crate::limine::Framebuffer;
use spin::Mutex;

const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;

/// Integer square root (Newton's method), used by `fill_circle` -- no `sqrt`
/// in `core` for a `no_std`/no-`libm` build.
fn isqrt(n: i64) -> i64 {
    if n <= 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

pub struct Console {
    fb: *mut u8,
    width: usize,
    height: usize,
    pitch: usize,
    bytes_per_pixel: usize,
    red_shift: u8,
    red_size: u8,
    green_shift: u8,
    green_size: u8,
    blue_shift: u8,
    blue_size: u8,
    cols: usize,
    rows: usize,
    cursor_col: usize,
    cursor_row: usize,
    fg: (u8, u8, u8),
    bg: (u8, u8, u8),
}

unsafe impl Send for Console {}

impl Console {
    /// # Safety
    /// `fb` must describe a valid, writable, memory-mapped linear framebuffer
    /// (true of the Limine framebuffer response once HHDM is mapped, which it
    /// is by the time the bootloader hands control to us).
    unsafe fn new(fb: &Framebuffer) -> Self {
        let width = fb.width as usize;
        let height = fb.height as usize;
        Self {
            fb: fb.address,
            width,
            height,
            pitch: fb.pitch as usize,
            bytes_per_pixel: (fb.bpp as usize) / 8,
            red_shift: fb.red_mask_shift,
            red_size: fb.red_mask_size,
            green_shift: fb.green_mask_shift,
            green_size: fb.green_mask_size,
            blue_shift: fb.blue_mask_shift,
            blue_size: fb.blue_mask_size,
            cols: width / GLYPH_W,
            rows: height / GLYPH_H,
            cursor_col: 0,
            cursor_row: 0,
            fg: (0xE0, 0xE0, 0xE0),
            bg: (0x10, 0x14, 0x1C),
        }
    }

    fn pack(&self, r: u8, g: u8, b: u8) -> u32 {
        let scale = |v: u8, size: u8| -> u32 {
            if size == 0 {
                0
            } else {
                (v as u32) >> (8 - size as u32)
            }
        };
        (scale(r, self.red_size) << self.red_shift)
            | (scale(g, self.green_size) << self.green_shift)
            | (scale(b, self.blue_size) << self.blue_shift)
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Pixel-precise rectangle fill, for GUI use (unlike the character-grid
    /// text console, which only ever addresses whole 8x8 cells).
    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: (u8, u8, u8)) {
        let packed = self.pack(color.0, color.1, color.2);
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = ((x as i64 + w as i64).max(0) as usize).min(self.width);
        let y1 = ((y as i64 + h as i64).max(0) as usize).min(self.height);
        for py in y0..y1 {
            for px in x0..x1 {
                self.put_pixel(px, py, packed);
            }
        }
    }

    /// Pixel-precise single-glyph draw. `bg` of `None` leaves background
    /// pixels untouched (useful for overlaying text on something already
    /// drawn, e.g. a title bar).
    pub fn draw_char_at(
        &mut self,
        x: i32,
        y: i32,
        ch: u8,
        fg: (u8, u8, u8),
        bg: Option<(u8, u8, u8)>,
    ) {
        let glyph = if (ch as usize) < FONT8X8.len() {
            &FONT8X8[ch as usize]
        } else {
            &FONT8X8[b'?' as usize]
        };
        let fg_color = self.pack(fg.0, fg.1, fg.2);
        let bg_color = bg.map(|c| self.pack(c.0, c.1, c.2));
        for (dy, bits) in glyph.iter().enumerate() {
            let py = y + dy as i32;
            if py < 0 || py as usize >= self.height {
                continue;
            }
            for dx in 0..GLYPH_W {
                let px = x + dx as i32;
                if px < 0 || px as usize >= self.width {
                    continue;
                }
                if (bits >> dx) & 1 != 0 {
                    self.put_pixel(px as usize, py as usize, fg_color);
                } else if let Some(bg_color) = bg_color {
                    self.put_pixel(px as usize, py as usize, bg_color);
                }
            }
        }
    }

    /// Pixel-precise string draw, left-to-right, 8px advance per character.
    pub fn draw_str_at(
        &mut self,
        x: i32,
        y: i32,
        s: &str,
        fg: (u8, u8, u8),
        bg: Option<(u8, u8, u8)>,
    ) {
        for (i, byte) in s.bytes().enumerate() {
            self.draw_char_at(x + (i * GLYPH_W) as i32, y, byte, fg, bg);
        }
    }

    /// Unpacks the raw pixel at (x, y) back into 8-bit RGB. The inverse of
    /// `pack`, used for alpha blending against whatever is already on screen.
    fn get_pixel(&self, x: usize, y: usize) -> (u8, u8, u8) {
        if x >= self.width || y >= self.height {
            return (0, 0, 0);
        }
        let offset = y * self.pitch + x * self.bytes_per_pixel;
        let raw: u32 = unsafe {
            let ptr = self.fb.add(offset);
            match self.bytes_per_pixel {
                4 => core::ptr::read_volatile(ptr as *const u32),
                3 => {
                    let b0 = core::ptr::read_volatile(ptr) as u32;
                    let b1 = core::ptr::read_volatile(ptr.add(1)) as u32;
                    let b2 = core::ptr::read_volatile(ptr.add(2)) as u32;
                    b0 | (b1 << 8) | (b2 << 16)
                }
                2 => core::ptr::read_volatile(ptr as *const u16) as u32,
                _ => 0,
            }
        };
        let unscale = |shift: u8, size: u8| -> u8 {
            if size == 0 {
                0
            } else {
                (((raw >> shift) & ((1u32 << size) - 1)) << (8 - size as u32)) as u8
            }
        };
        (
            unscale(self.red_shift, self.red_size),
            unscale(self.green_shift, self.green_size),
            unscale(self.blue_shift, self.blue_size),
        )
    }

    /// Alpha-blends `color` over whatever is already on screen in the given
    /// rect (0 = fully transparent no-op, 255 = opaque, same as `fill_rect`).
    /// The glassy/translucent look of the top bar, dock, and desktop panels
    /// all come from this rather than any real compositing buffer.
    pub fn blend_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: (u8, u8, u8), alpha: u8) {
        if alpha == 0 {
            return;
        }
        if alpha == 255 {
            self.fill_rect(x, y, w, h, color);
            return;
        }
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = ((x as i64 + w as i64).max(0) as usize).min(self.width);
        let y1 = ((y as i64 + h as i64).max(0) as usize).min(self.height);
        let a = u32::from(alpha);
        let mix =
            |c: u8, b: u8| -> u8 { ((u32::from(c) * a + u32::from(b) * (255 - a)) / 255) as u8 };
        for py in y0..y1 {
            for px in x0..x1 {
                let (br, bg, bb) = self.get_pixel(px, py);
                let blended = (mix(color.0, br), mix(color.1, bg), mix(color.2, bb));
                let packed = self.pack(blended.0, blended.1, blended.2);
                self.put_pixel(px, py, packed);
            }
        }
    }

    /// Filled circle via horizontal scanline spans (integer-sqrt half-widths
    /// per row) -- used for the moon and the top bar's crescent logo.
    pub fn fill_circle(&mut self, cx: i32, cy: i32, radius: i32, color: (u8, u8, u8)) {
        if radius <= 0 {
            return;
        }
        let packed = self.pack(color.0, color.1, color.2);
        for dy in -radius..=radius {
            let py = cy + dy;
            if py < 0 || py as usize >= self.height {
                continue;
            }
            let span = isqrt((radius * radius - dy * dy).max(0) as i64) as i32;
            let x0 = (cx - span).max(0) as usize;
            let x1 = ((cx + span + 1).max(0) as usize).min(self.width);
            for px in x0..x1 {
                self.put_pixel(px, py as usize, packed);
            }
        }
    }

    fn put_pixel(&mut self, x: usize, y: usize, color: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = y * self.pitch + x * self.bytes_per_pixel;
        unsafe {
            let ptr = self.fb.add(offset);
            match self.bytes_per_pixel {
                4 => core::ptr::write_volatile(ptr as *mut u32, color),
                3 => {
                    core::ptr::write_volatile(ptr, color as u8);
                    core::ptr::write_volatile(ptr.add(1), (color >> 8) as u8);
                    core::ptr::write_volatile(ptr.add(2), (color >> 16) as u8);
                }
                2 => core::ptr::write_volatile(ptr as *mut u16, color as u16),
                _ => {}
            }
        }
    }

    fn draw_glyph(&mut self, ch: u8, col: usize, row: usize) {
        let glyph = if (ch as usize) < FONT8X8.len() {
            &FONT8X8[ch as usize]
        } else {
            &FONT8X8[b'?' as usize]
        };
        let (fr, fg, fb) = self.fg;
        let (br, bg, bb) = self.bg;
        let fg_color = self.pack(fr, fg, fb);
        let bg_color = self.pack(br, bg, bb);
        let base_x = col * GLYPH_W;
        let base_y = row * GLYPH_H;
        for (dy, bits) in glyph.iter().enumerate() {
            for dx in 0..GLYPH_W {
                let on = (bits >> dx) & 1 != 0;
                let color = if on { fg_color } else { bg_color };
                self.put_pixel(base_x + dx, base_y + dy, color);
            }
        }
    }

    fn clear_row(&mut self, row: usize) {
        for col in 0..self.cols {
            self.draw_glyph(b' ', col, row);
        }
    }

    fn scroll(&mut self) {
        let row_bytes = self.pitch * GLYPH_H;
        let total_rows_px = self.rows * GLYPH_H;
        unsafe {
            core::ptr::copy(
                self.fb.add(row_bytes),
                self.fb,
                self.pitch * (total_rows_px - GLYPH_H),
            );
        }
        self.clear_row(self.rows - 1);
    }

    fn newline(&mut self) {
        self.cursor_col = 0;
        if self.cursor_row + 1 >= self.rows {
            self.scroll();
        } else {
            self.cursor_row += 1;
        }
    }

    pub fn write_str(&mut self, s: &str) {
        for byte in s.bytes() {
            match byte {
                b'\n' => self.newline(),
                b'\r' => self.cursor_col = 0,
                _ => {
                    if self.cursor_col >= self.cols {
                        self.newline();
                    }
                    self.draw_glyph(byte, self.cursor_col, self.cursor_row);
                    self.cursor_col += 1;
                }
            }
        }
    }
}

impl core::fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        Console::write_str(self, s);
        Ok(())
    }
}

pub static CONSOLE: Mutex<Option<Console>> = Mutex::new(None);

/// # Safety
/// See [`Console::new`]; must only be called once, with the framebuffer
/// response Limine gave us.
pub unsafe fn init(fb: &Framebuffer) {
    let mut console = unsafe { Console::new(fb) };
    for row in 0..console.rows {
        console.clear_row(row);
    }
    *CONSOLE.lock() = Some(console);
}

/// Runs `f` with exclusive access to the console/framebuffer, if one was
/// initialized. Used by the GUI compositor to batch a whole redraw under a
/// single lock acquisition.
pub fn with<R>(f: impl FnOnce(&mut Console) -> R) -> Option<R> {
    CONSOLE.lock().as_mut().map(f)
}

#[doc(hidden)]
pub fn _print(args: core::fmt::Arguments) {
    use core::fmt::Write;
    if let Some(console) = CONSOLE.lock().as_mut() {
        console.write_fmt(args).ok();
    }
}

#[macro_export]
macro_rules! fb_print {
    ($($arg:tt)*) => {
        $crate::framebuffer::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! fb_println {
    () => { $crate::framebuffer::_print(format_args!("\n")) };
    ($($arg:tt)*) => {{
        $crate::framebuffer::_print(format_args!($($arg)*));
        $crate::framebuffer::_print(format_args!("\n"));
    }};
}
