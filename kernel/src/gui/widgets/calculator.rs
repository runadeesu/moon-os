//! An integer calculator -- real arithmetic (+ - * /), not a display prop.
//! Integer-only is a deliberate choice, not a shortcut: this kernel's
//! context-switch path never saves/restores FPU/SSE state (see the
//! `gui/desktop.rs` doc comment), so using floats anywhere risks silent
//! corruption the moment two tasks touch floating-point state. A real
//! calculator that only handles whole numbers is honest about that limit
//! instead of pretending to support decimals and quietly breaking.

use crate::framebuffer;
use alloc::format;
use alloc::string::String;

const BTN_W: i32 = 44;
const BTN_H: i32 = 28;
const BTN_GAP: i32 = 4;
const GRID_TOP: i32 = 40;

const LABELS: [[&str; 4]; 4] = [
    ["7", "8", "9", "/"],
    ["4", "5", "6", "*"],
    ["1", "2", "3", "-"],
    ["0", "C", "=", "+"],
];

pub struct CalculatorState {
    display: String,
    accumulator: i64,
    pending_op: Option<char>,
    fresh_entry: bool,
    error: bool,
}

impl CalculatorState {
    pub fn new() -> Self {
        Self {
            display: String::from("0"),
            accumulator: 0,
            pending_op: None,
            fresh_entry: true,
            error: false,
        }
    }

    fn current_value(&self) -> i64 {
        self.display.parse().unwrap_or(0)
    }

    fn apply(lhs: i64, op: char, rhs: i64) -> Option<i64> {
        match op {
            '+' => lhs.checked_add(rhs),
            '-' => lhs.checked_sub(rhs),
            '*' => lhs.checked_mul(rhs),
            '/' => lhs.checked_div(rhs),
            _ => None,
        }
    }

    fn press(&mut self, label: &str) {
        if self.error && label != "C" {
            return;
        }
        match label {
            "C" => {
                *self = Self::new();
            }
            "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => {
                let digit = label;
                if self.fresh_entry || self.display == "0" {
                    self.display = String::from(digit);
                    self.fresh_entry = false;
                } else {
                    self.display.push_str(digit);
                }
            }
            "+" | "-" | "*" | "/" => {
                let value = self.current_value();
                let result = match self.pending_op {
                    Some(op) => Self::apply(self.accumulator, op, value),
                    None => Some(value),
                };
                match result {
                    Some(r) => {
                        self.accumulator = r;
                        self.display = format!("{r}");
                    }
                    None => {
                        self.error = true;
                        self.display = String::from("Error");
                    }
                }
                self.pending_op = Some(label.chars().next().unwrap());
                self.fresh_entry = true;
            }
            "=" => {
                let value = self.current_value();
                if let Some(op) = self.pending_op.take() {
                    match Self::apply(self.accumulator, op, value) {
                        Some(r) => {
                            self.accumulator = r;
                            self.display = format!("{r}");
                        }
                        None => {
                            self.error = true;
                            self.display = String::from("Error");
                        }
                    }
                }
                self.fresh_entry = true;
            }
            _ => {}
        }
    }

    /// `x`/`y` are local to the widget's content area, same convention as
    /// `Window::handle_click`.
    pub fn handle_click(&mut self, x: i32, y: i32) {
        if y < GRID_TOP {
            return;
        }
        let row = (y - GRID_TOP) / (BTN_H + BTN_GAP);
        let col = x / (BTN_W + BTN_GAP);
        if let (Ok(row), Ok(col)) = (usize::try_from(row), usize::try_from(col)) {
            if let Some(label) = LABELS.get(row).and_then(|r| r.get(col)) {
                self.press(label);
            }
        }
    }

    pub fn render(&self, x: i32, y: i32, w: u32, _h: u32) {
        framebuffer::with(|c| {
            c.fill_rect(x, y, w, GRID_TOP as u32, (0x0C, 0x0C, 0x10));
            let max_chars = ((w as i32 - 8) / 8).max(1) as usize;
            let text = if self.display.len() > max_chars {
                &self.display[self.display.len() - max_chars..]
            } else {
                self.display.as_str()
            };
            c.draw_str_at(x + 4, y + 16, text, (0x80, 0xE8, 0xA0), None);

            for (row, labels) in LABELS.iter().enumerate() {
                for (col, label) in labels.iter().enumerate() {
                    let bx = x + col as i32 * (BTN_W + BTN_GAP);
                    let by = y + GRID_TOP + row as i32 * (BTN_H + BTN_GAP);
                    let is_op = matches!(*label, "+" | "-" | "*" | "/" | "=");
                    let bg = if is_op {
                        (0x1A, 0x3A, 0x46)
                    } else {
                        (0x20, 0x20, 0x28)
                    };
                    c.fill_rect(bx, by, BTN_W as u32, BTN_H as u32, bg);
                    c.draw_str_at(
                        bx + BTN_W / 2 - 4,
                        by + BTN_H / 2 - 4,
                        label,
                        (0xE0, 0xE0, 0xE0),
                        None,
                    );
                }
            }
        });
    }
}

impl Default for CalculatorState {
    fn default() -> Self {
        Self::new()
    }
}
