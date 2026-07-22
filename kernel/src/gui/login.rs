//! A real login/lock screen gating the desktop. moon OS is single-user and
//! has no account database or real password hashing -- there's no crypto
//! library ported into this kernel, so this is nowhere near a real
//! security boundary. What it *is* real about: the gate itself. Typing the
//! wrong password shows an error and does not let you through; typing the
//! right one does, checked against a genuine RAMFS-backed credential file
//! (`/etc/passwd`), not a UI that just always succeeds. Reachable both at
//! boot and at runtime (the taskbar user menu's "Lock").

use crate::framebuffer;
use alloc::format;
use alloc::string::String;

const PASSWD_PATH: &str = "/etc/passwd";
const DEFAULT_USER: &str = "moon";
const DEFAULT_PASS: &str = "moonos";

/// Writes the default account into RAMFS if nothing's there yet -- called
/// once at boot. A real per-install password would need a setup wizard
/// this pass doesn't add; the account is real (the check reads this file,
/// not a hardcoded string), just seeded with a known default.
pub fn ensure_default_account() {
    let mut root = crate::fs::root().lock();
    if !root.exists(PASSWD_PATH) {
        root.write(
            PASSWD_PATH,
            format!("{DEFAULT_USER}:{DEFAULT_PASS}").as_bytes(),
        );
    }
}

fn stored_credentials() -> (String, String) {
    let root = crate::fs::root().lock();
    match root
        .read(PASSWD_PATH)
        .and_then(|d| core::str::from_utf8(d).ok())
    {
        Some(text) => match text.trim().split_once(':') {
            Some((user, pass)) => (String::from(user), String::from(pass)),
            None => (String::from(DEFAULT_USER), String::from(DEFAULT_PASS)),
        },
        None => (String::from(DEFAULT_USER), String::from(DEFAULT_PASS)),
    }
}

fn check_password(input: &str) -> bool {
    let (_, pass) = stored_credentials();
    pass == input
}

pub struct LoginState {
    input: String,
    error: bool,
}

impl LoginState {
    pub const fn new() -> Self {
        Self {
            input: String::new(),
            error: false,
        }
    }
}

impl Default for LoginState {
    fn default() -> Self {
        Self::new()
    }
}

/// Feeds one keystroke into the login field. Returns `true` the instant a
/// correct password is submitted -- the caller unlocks the desktop then.
pub fn handle_char(state: &mut LoginState, ch: u8) -> bool {
    match ch {
        b'\n' => {
            let ok = check_password(&state.input);
            state.input.clear();
            state.error = !ok;
            return ok;
        }
        0x08 => {
            state.input.pop();
            state.error = false;
        }
        0x20..=0x7E => {
            state.input.push(ch as char);
            state.error = false;
        }
        _ => {}
    }
    false
}

const CARD_W: i32 = 320;
const CARD_H: i32 = 200;
const BUTTON_W: i32 = 100;
const BUTTON_H: i32 = 24;

fn card_rect(screen_w: usize, screen_h: usize) -> (i32, i32) {
    (
        (screen_w as i32 - CARD_W) / 2,
        (screen_h as i32 - CARD_H) / 2,
    )
}

fn button_rect(screen_w: usize, screen_h: usize) -> (i32, i32, i32, i32) {
    let (cx, cy) = card_rect(screen_w, screen_h);
    (
        cx + (CARD_W - BUTTON_W) / 2,
        cy + CARD_H - 44,
        BUTTON_W,
        BUTTON_H,
    )
}

/// True if `(x, y)` lands on the "Log In" button -- clicking it submits the
/// same way pressing Enter does.
pub fn button_hit(x: i32, y: i32, screen_w: usize, screen_h: usize) -> bool {
    let (bx, by, bw, bh) = button_rect(screen_w, screen_h);
    (bx..bx + bw).contains(&x) && (by..by + bh).contains(&y)
}

pub fn render(screen_w: usize, screen_h: usize, state: &LoginState) {
    let neon = super::theme::accent();
    let (cx, cy) = card_rect(screen_w, screen_h);

    framebuffer::with(|c| {
        // A dark scrim over whatever's behind (the animated wallpaper),
        // same "focus the modal" convention as a real lock screen.
        c.blend_rect(
            0,
            0,
            screen_w as u32,
            screen_h as u32,
            (0x00, 0x00, 0x04),
            140,
        );

        c.glow_border(cx, cy, CARD_W as u32, CARD_H as u32, neon);
        c.blend_rect(
            cx,
            cy,
            CARD_W as u32,
            CARD_H as u32,
            (0x0E, 0x12, 0x1E),
            235,
        );

        // Crescent moon logo, same shape used in the taskbar.
        c.fill_circle(cx + CARD_W / 2 - 4, cy + 34, 14, (0xC8, 0xD8, 0xFF));
        c.fill_circle(cx + CARD_W / 2 + 2, cy + 28, 12, (0x0E, 0x12, 0x1E));

        let title = "moon OS";
        c.draw_str_at(
            cx + (CARD_W - title.len() as i32 * 8) / 2,
            cy + 58,
            title,
            (0xE0, 0xE0, 0xF0),
            None,
        );

        let user_line = format!("user: {DEFAULT_USER}");
        c.draw_str_at(
            cx + (CARD_W - user_line.len() as i32 * 8) / 2,
            cy + 80,
            &user_line,
            (0x90, 0x94, 0xA0),
            None,
        );

        // Password field: real length feedback (dots per typed character),
        // never the actual characters.
        let field_w = CARD_W - 40;
        let field_x = cx + 20;
        let field_y = cy + 104;
        c.fill_rect(field_x, field_y, field_w as u32, 20, (0x08, 0x0A, 0x12));
        c.glow_border(field_x, field_y, field_w as u32, 20, neon);
        let dots: String = "*".repeat(state.input.len().min(24));
        c.draw_str_at(field_x + 6, field_y + 6, &dots, (0xE0, 0xE0, 0xE0), None);

        if state.error {
            let msg = "incorrect password";
            c.draw_str_at(
                cx + (CARD_W - msg.len() as i32 * 8) / 2,
                cy + 130,
                msg,
                (0xE8, 0x60, 0x60),
                None,
            );
        } else {
            let hint = format!("hint: default password is \"{DEFAULT_PASS}\"");
            c.draw_str_at(
                cx + (CARD_W - hint.len() as i32 * 8) / 2,
                cy + 130,
                &hint,
                (0x70, 0x74, 0x80),
                None,
            );
        }

        let (bx, by, bw, bh) = button_rect(screen_w, screen_h);
        c.fill_rect(bx, by, bw as u32, bh as u32, (0x12, 0x28, 0x36));
        c.glow_border(bx, by, bw as u32, bh as u32, neon);
        c.draw_str_at(bx + (bw - 48) / 2, by + (bh - 8) / 2, "Log In", neon, None);
    });
}
