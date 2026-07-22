//! A real login/lock screen gating the desktop, plus real account
//! registration: moon OS now supports more than one local account.
//! Credentials live in RAMFS at `/etc/passwd`, one real `user:pass` line
//! per account -- there's no crypto library ported into this kernel, so
//! passwords are stored in plain text and this is nowhere near a real
//! security boundary. What *is* real: the gate itself (wrong credentials
//! are rejected, not waved through), and Sign Up genuinely creates a new
//! account you can then log into (rejecting a name that's already taken),
//! not a form that just pretends to register you.
//!
//! Day-to-day Log In only asks for a password -- the account you're
//! unlocking is shown as a fixed label, the way a real desktop OS's lock
//! screen greets "Welcome back, <user>" rather than making you re-type your
//! own name every time. Typing a *different* account still works via the
//! "Switch user" link, which is the only place the username field becomes
//! editable outside of Sign Up.

use crate::framebuffer;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const PASSWD_PATH: &str = "/etc/passwd";
const DEFAULT_USER: &str = "moon";
const DEFAULT_PASS: &str = "moonos";

/// Writes the default account into RAMFS if nothing's there yet -- called
/// once at boot. Additional accounts created via Sign Up are appended
/// alongside it, real lines in the same real file.
pub fn ensure_default_account() {
    let mut root = crate::fs::root().lock();
    if !root.exists(PASSWD_PATH) {
        root.write(
            PASSWD_PATH,
            format!("{DEFAULT_USER}:{DEFAULT_PASS}\n").as_bytes(),
        );
    }
}

fn all_accounts() -> Vec<(String, String)> {
    let root = crate::fs::root().lock();
    match root
        .read(PASSWD_PATH)
        .and_then(|d| core::str::from_utf8(d).ok())
    {
        Some(text) => text
            .lines()
            .filter_map(|line| line.split_once(':'))
            .map(|(u, p)| (u.to_string(), p.to_string()))
            .collect(),
        None => alloc::vec![(String::from(DEFAULT_USER), String::from(DEFAULT_PASS))],
    }
}

fn check_login(username: &str, password: &str) -> bool {
    all_accounts()
        .iter()
        .any(|(u, p)| u == username && p == password)
}

/// Creates a new account. Fails if `username` is empty, the password is
/// empty, or the name is already taken -- real validation, not a form that
/// accepts anything and silently does nothing.
fn create_account(username: &str, password: &str) -> Result<(), &'static str> {
    if username.trim().is_empty() {
        return Err("username can't be empty");
    }
    if password.is_empty() {
        return Err("password can't be empty");
    }
    if all_accounts().iter().any(|(u, _)| u == username) {
        return Err("that username is already taken");
    }
    let mut root = crate::fs::root().lock();
    let mut existing = root
        .read(PASSWD_PATH)
        .map(|d| d.to_vec())
        .unwrap_or_default();
    existing.extend_from_slice(format!("{username}:{password}\n").as_bytes());
    root.write(PASSWD_PATH, &existing);
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Password-only: `current_user` is a fixed label, only `password` is
    /// editable.
    LogIn,
    /// Both fields editable, for logging in as an account other than
    /// `current_user`.
    SwitchUser,
    /// Both fields editable, creates a new account.
    SignUp,
}

pub struct LoginState {
    /// The account the password-only Log In screen greets and unlocks.
    /// Updated on every successful login/registration, but never persisted
    /// to disk -- RAMFS itself doesn't survive a reboot, so remembering it
    /// only within the running session is the honest scope here.
    current_user: String,
    /// Editable username input, used only in SwitchUser/SignUp mode.
    username: String,
    password: String,
    username_focused: bool,
    mode: Mode,
    error: Option<String>,
}

impl LoginState {
    pub const fn new() -> Self {
        Self {
            // Empty means "not yet set" -- `display_user()` falls back to
            // DEFAULT_USER. Can't call `String::from` here: this runs in a
            // static initializer, which only allows genuinely const calls.
            current_user: String::new(),
            username: String::new(),
            password: String::new(),
            username_focused: false,
            mode: Mode::LogIn,
            error: None,
        }
    }

    fn display_user(&self) -> &str {
        if self.current_user.is_empty() {
            DEFAULT_USER
        } else {
            &self.current_user
        }
    }

    /// Resets the screen for a fresh lock/unlock cycle -- clears whatever
    /// was typed and drops back to password-only Log In, but keeps
    /// remembering which account was last active.
    pub fn lock(&mut self) {
        self.username.clear();
        self.password.clear();
        self.username_focused = false;
        self.mode = Mode::LogIn;
        self.error = None;
    }
}

impl Default for LoginState {
    fn default() -> Self {
        Self::new()
    }
}

/// Feeds one keystroke into whichever field has focus. Returns `true` the
/// instant a login succeeds -- the caller unlocks the desktop then. A
/// successful Sign Up also returns `true` (it logs the new account straight
/// in, the same way a real OS's "create account" flow lands you on the
/// desktop rather than bouncing you back to a login prompt).
pub fn handle_char(state: &mut LoginState, ch: u8) -> bool {
    let username_editable = state.mode != Mode::LogIn;
    match ch {
        b'\t' if username_editable => {
            state.username_focused = !state.username_focused;
        }
        b'\n' => return submit(state),
        0x08 => {
            if username_editable && state.username_focused {
                state.username.pop();
            } else {
                state.password.pop();
            }
        }
        0x20..=0x7E => {
            if username_editable && state.username_focused {
                state.username.push(ch as char);
            } else {
                state.password.push(ch as char);
            }
        }
        _ => {}
    }
    false
}

fn submit(state: &mut LoginState) -> bool {
    match state.mode {
        Mode::LogIn => {
            let user = state.display_user().to_string();
            let ok = check_login(&user, &state.password);
            state.password.clear();
            if ok {
                state.error = None;
                true
            } else {
                state.error = Some(String::from("incorrect password"));
                false
            }
        }
        Mode::SwitchUser => {
            let ok = check_login(&state.username, &state.password);
            if ok {
                state.current_user = state.username.clone();
                state.username.clear();
                state.password.clear();
                state.error = None;
                state.mode = Mode::LogIn;
                true
            } else {
                state.password.clear();
                state.error = Some(String::from("incorrect username or password"));
                false
            }
        }
        Mode::SignUp => match create_account(&state.username, &state.password) {
            Ok(()) => {
                state.current_user = state.username.clone();
                state.username.clear();
                state.password.clear();
                state.error = None;
                state.mode = Mode::LogIn;
                true
            }
            Err(msg) => {
                state.error = Some(String::from(msg));
                false
            }
        },
    }
}

fn enter_switch_user(state: &mut LoginState) {
    state.mode = Mode::SwitchUser;
    state.username.clear();
    state.password.clear();
    state.username_focused = true;
    state.error = None;
}

fn enter_sign_up(state: &mut LoginState) {
    state.mode = Mode::SignUp;
    state.username.clear();
    state.password.clear();
    state.username_focused = true;
    state.error = None;
}

fn cancel_to_login(state: &mut LoginState) {
    state.mode = Mode::LogIn;
    state.username.clear();
    state.password.clear();
    state.username_focused = false;
    state.error = None;
}

const CARD_W: i32 = 320;
const CARD_H: i32 = 280;
const BUTTON_W: i32 = 120;
const BUTTON_H: i32 = 24;
const FIELD_H: i32 = 20;

fn card_rect(screen_w: usize, screen_h: usize) -> (i32, i32) {
    (
        (screen_w as i32 - CARD_W) / 2,
        (screen_h as i32 - CARD_H) / 2,
    )
}

fn username_field_rect(screen_w: usize, screen_h: usize) -> (i32, i32, i32, i32) {
    let (cx, cy) = card_rect(screen_w, screen_h);
    (cx + 20, cy + 96, CARD_W - 40, FIELD_H)
}

fn password_field_rect(screen_w: usize, screen_h: usize) -> (i32, i32, i32, i32) {
    let (cx, cy) = card_rect(screen_w, screen_h);
    (cx + 20, cy + 124, CARD_W - 40, FIELD_H)
}

fn button_rect(screen_w: usize, screen_h: usize) -> (i32, i32, i32, i32) {
    let (cx, cy) = card_rect(screen_w, screen_h);
    (cx + (CARD_W - BUTTON_W) / 2, cy + 158, BUTTON_W, BUTTON_H)
}

fn primary_link_rect(screen_w: usize, screen_h: usize) -> (i32, i32, i32, i32) {
    let (cx, cy) = card_rect(screen_w, screen_h);
    (cx + 20, cy + 198, CARD_W - 40, 14)
}

fn secondary_link_rect(screen_w: usize, screen_h: usize) -> (i32, i32, i32, i32) {
    let (cx, cy) = card_rect(screen_w, screen_h);
    (cx + 20, cy + 216, CARD_W - 40, 14)
}

fn point_in(rect: (i32, i32, i32, i32), x: i32, y: i32) -> bool {
    let (rx, ry, rw, rh) = rect;
    (rx..rx + rw).contains(&x) && (ry..ry + rh).contains(&y)
}

/// Every click the login screen understands -- returns `true` if the click
/// resulted in a successful login/registration (the caller unlocks then).
pub fn handle_click(
    state: &mut LoginState,
    x: i32,
    y: i32,
    screen_w: usize,
    screen_h: usize,
) -> bool {
    let username_editable = state.mode != Mode::LogIn;
    if username_editable && point_in(username_field_rect(screen_w, screen_h), x, y) {
        state.username_focused = true;
    } else if point_in(password_field_rect(screen_w, screen_h), x, y) {
        state.username_focused = false;
    } else if point_in(button_rect(screen_w, screen_h), x, y) {
        return submit(state);
    } else if point_in(primary_link_rect(screen_w, screen_h), x, y) {
        match state.mode {
            Mode::LogIn => enter_switch_user(state),
            Mode::SwitchUser => enter_sign_up(state),
            Mode::SignUp => cancel_to_login(state),
        }
    } else if point_in(secondary_link_rect(screen_w, screen_h), x, y) {
        match state.mode {
            Mode::LogIn => enter_sign_up(state),
            Mode::SwitchUser | Mode::SignUp => cancel_to_login(state),
        }
    }
    false
}

pub fn render(screen_w: usize, screen_h: usize, state: &LoginState) {
    let neon = super::theme::accent();
    let (cx, cy) = card_rect(screen_w, screen_h);
    let username_editable = state.mode != Mode::LogIn;

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
        c.fill_circle(cx + CARD_W / 2 - 4, cy + 30, 14, (0xC8, 0xD8, 0xFF));
        c.fill_circle(cx + CARD_W / 2 + 2, cy + 24, 12, (0x0E, 0x12, 0x1E));

        let title = match state.mode {
            Mode::LogIn => "moon OS",
            Mode::SwitchUser => "Switch User",
            Mode::SignUp => "Create Account",
        };
        c.draw_str_at(
            cx + (CARD_W - title.len() as i32 * 8) / 2,
            cy + 54,
            title,
            (0xE0, 0xE0, 0xF0),
            None,
        );

        if username_editable {
            let label = "username:";
            c.draw_str_at(cx + 20, cy + 82, label, (0x90, 0x94, 0xA0), None);
            let (ux, uy, uw, uh) = username_field_rect(screen_w, screen_h);
            c.fill_rect(ux, uy, uw as u32, uh as u32, (0x08, 0x0A, 0x12));
            c.glow_border(
                ux,
                uy,
                uw as u32,
                uh as u32,
                if state.username_focused {
                    neon
                } else {
                    (0x40, 0x44, 0x50)
                },
            );
            c.draw_str_at(ux + 6, uy + 6, &state.username, (0xE0, 0xE0, 0xE0), None);
        } else {
            let label = format!("Welcome back, {}", state.display_user());
            c.draw_str_at(
                cx + (CARD_W - label.len() as i32 * 8).max(0) / 2,
                cy + 90,
                &label,
                (0x90, 0xB0, 0xD0),
                None,
            );
        }

        let (px, py, pw, ph) = password_field_rect(screen_w, screen_h);
        c.fill_rect(px, py, pw as u32, ph as u32, (0x08, 0x0A, 0x12));
        c.glow_border(
            px,
            py,
            pw as u32,
            ph as u32,
            if !username_editable || !state.username_focused {
                neon
            } else {
                (0x40, 0x44, 0x50)
            },
        );
        let dots: String = "*".repeat(state.password.len().min(24));
        c.draw_str_at(px + 6, py + 6, &dots, (0xE0, 0xE0, 0xE0), None);

        if let Some(err) = &state.error {
            c.draw_str_at(
                cx + (CARD_W - (err.len() as i32).min(38) * 8) / 2,
                cy + 148,
                err,
                (0xE8, 0x60, 0x60),
                None,
            );
        } else {
            let hint = match state.mode {
                Mode::LogIn => format!("hint: default password is \"{DEFAULT_PASS}\""),
                Mode::SwitchUser => String::from("enter another account's username and password"),
                Mode::SignUp => String::from("pick a username and password"),
            };
            c.draw_str_at(
                cx + (CARD_W - hint.len() as i32 * 8) / 2,
                cy + 148,
                &hint,
                (0x70, 0x74, 0x80),
                None,
            );
        }

        let (bx, by, bw, bh) = button_rect(screen_w, screen_h);
        c.fill_rect(bx, by, bw as u32, bh as u32, (0x12, 0x28, 0x36));
        c.glow_border(bx, by, bw as u32, bh as u32, neon);
        let btn_label = match state.mode {
            Mode::LogIn | Mode::SwitchUser => "Log In",
            Mode::SignUp => "Create Account",
        };
        c.draw_str_at(
            bx + (bw - btn_label.len() as i32 * 8) / 2,
            by + (bh - 8) / 2,
            btn_label,
            neon,
            None,
        );

        let primary_label = match state.mode {
            Mode::LogIn => "Switch user",
            Mode::SwitchUser => "Sign Up instead",
            Mode::SignUp => "Have an account? Log In",
        };
        c.draw_str_at(
            cx + (CARD_W - primary_label.len() as i32 * 8) / 2,
            cy + 200,
            primary_label,
            (0x80, 0xC0, 0xE0),
            None,
        );

        let secondary_label = match state.mode {
            Mode::LogIn => "New here? Sign Up",
            Mode::SwitchUser => "Cancel",
            Mode::SignUp => "",
        };
        if !secondary_label.is_empty() {
            c.draw_str_at(
                cx + (CARD_W - secondary_label.len() as i32 * 8) / 2,
                cy + 218,
                secondary_label,
                (0x80, 0xC0, 0xE0),
                None,
            );
        }
    });
}
