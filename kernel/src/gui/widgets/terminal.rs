//! A built-in terminal widget: a scrollback buffer, a single editable input
//! line, and a real shell over the RAMFS (`ls`/`cd`/`pwd`/`mkdir`/`touch`/
//! `rm`/`cp`/`mv`/`cat`/`echo`, with real `>` output redirection into RAMFS
//! files, and real pipes: `grep`/`sort`/`wc`/`head`/`tail` each genuinely
//! consume the previous stage's output lines, not a simulated connection).
//! Not a real process running through the ELF loader -- these are kernel-
//! hosted builtins, not `/bin/ls` -- but every operation is real: `cat`/
//! `ls`/`cd` genuinely read the same `fs::root()` RAMFS the File Manager and
//! package manager use, `rm`/`mkdir`/`cp`/`mv` genuinely mutate it.
//! Tab-completion and arrow-key history are real too. Output is
//! color-coded by kind (errors/success/directories/etc.), not a flat
//! single color.

use crate::framebuffer;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const MAX_LINES: usize = 200;
const LINE_HEIGHT: i32 = 10;

const COMMANDS: &[&str] = &[
    "help", "clear", "uptime", "mem", "echo", "pkg", "pwd", "cd", "ls", "mkdir", "touch", "rm",
    "cp", "mv", "cat", "whoami", "date", "time", "history", "grep", "sort", "wc", "head", "tail",
    "reboot", "shutdown",
];

/// A rendered line's color, chosen by what kind of output it is -- real
/// categorization (error vs. success vs. a directory entry vs. plain text),
/// not decoration.
#[derive(Clone, Copy)]
enum LineColor {
    Normal,
    Prompt,
    Error,
    Success,
    Dir,
    Accent,
}

impl LineColor {
    fn rgb(self) -> (u8, u8, u8) {
        match self {
            LineColor::Normal => (0xC0, 0xC0, 0xC0),
            LineColor::Prompt => (0x80, 0x84, 0x90),
            LineColor::Error => (0xE8, 0x60, 0x60),
            LineColor::Success => (0x60, 0xE8, 0x90),
            LineColor::Dir => (0x90, 0xC8, 0xFF),
            LineColor::Accent => (0xE0, 0xE0, 0x60),
        }
    }
}

pub struct TerminalState {
    lines: VecDeque<(String, LineColor)>,
    current: String,
    /// Previously entered commands, oldest first -- `Up`/`Down` walk this.
    history: Vec<String>,
    /// Index into `history` while browsing with `Up`/`Down`; `None` means
    /// the input line is fresh (not currently recalling a past command).
    history_pos: Option<usize>,
    cwd: String,
}

/// Resolves `path` against `cwd`: absolute paths (leading `/`) pass through,
/// anything else is joined on. Handles `.`/`..` segments so `cd ..` and
/// `cat ../foo` work, but doesn't attempt symlinks -- RAMFS doesn't have any.
fn resolve(cwd: &str, path: &str) -> String {
    let mut segments: Vec<&str> = if path.starts_with('/') {
        Vec::new()
    } else {
        cwd.split('/').filter(|s| !s.is_empty()).collect()
    };
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            seg => segments.push(seg),
        }
    }
    if segments.is_empty() {
        String::from("/")
    } else {
        format!("/{}", segments.join("/"))
    }
}

impl TerminalState {
    pub fn new() -> Self {
        let mut lines = VecDeque::new();
        lines.push_back((
            String::from("moon OS terminal -- type 'help'"),
            LineColor::Accent,
        ));
        Self {
            lines,
            current: String::new(),
            history: Vec::new(),
            history_pos: None,
            cwd: String::from("/"),
        }
    }

    pub fn handle_char(&mut self, ch: u8) {
        match ch {
            b'\n' => {
                let line = core::mem::take(&mut self.current);
                self.push_line(format!("{} > {}", self.cwd, line), LineColor::Prompt);
                if !line.trim().is_empty() {
                    self.history.push(line.clone());
                }
                self.history_pos = None;
                self.run_command(&line);
            }
            0x08 => {
                self.current.pop();
                self.history_pos = None;
            }
            b'\t' => self.complete(),
            0x20..=0x7E => {
                self.current.push(ch as char);
                self.history_pos = None;
            }
            _ => {}
        }
    }

    /// Completes the word under the cursor: the first word against
    /// `COMMANDS`, any later word against entries in `cwd`. Only completes
    /// when the match is unambiguous (a single candidate) -- listing every
    /// candidate on multiple matches would need more UI than a single input
    /// line has room for, so it's a no-op there rather than a half feature.
    fn complete(&mut self) {
        let is_first_word = !self.current.trim_start().contains(' ');
        let word_start = self.current.rfind(' ').map_or(0, |i| i + 1);
        let word = &self.current[word_start..];
        if word.is_empty() {
            return;
        }

        let candidates: Vec<String> = if is_first_word {
            COMMANDS
                .iter()
                .filter(|c| c.starts_with(word))
                .map(|c| c.to_string())
                .collect()
        } else {
            let root = crate::fs::root().lock();
            root.list_dir(&self.cwd)
                .into_iter()
                .map(|(path, is_dir)| {
                    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
                    if is_dir {
                        format!("{name}/")
                    } else {
                        name
                    }
                })
                .filter(|name| name.starts_with(word))
                .collect()
        };

        if candidates.len() == 1 {
            self.current.truncate(word_start);
            self.current.push_str(&candidates[0]);
        }
    }

    /// Arrow-key command-history recall: `Up` steps back to older
    /// commands, `Down` steps forward (and back to a blank line once past
    /// the newest recalled entry).
    pub fn handle_special_key(&mut self, key: super::super::SpecialKey) {
        use super::super::SpecialKey;
        if self.history.is_empty() {
            return;
        }
        match key {
            SpecialKey::Up => {
                let next = match self.history_pos {
                    Some(0) => 0,
                    Some(p) => p - 1,
                    None => self.history.len() - 1,
                };
                self.history_pos = Some(next);
                self.current = self.history[next].clone();
            }
            SpecialKey::Down => match self.history_pos {
                Some(p) if p + 1 < self.history.len() => {
                    self.history_pos = Some(p + 1);
                    self.current = self.history[p + 1].clone();
                }
                Some(_) => {
                    self.history_pos = None;
                    self.current.clear();
                }
                None => {}
            },
            _ => {}
        }
    }

    fn push_line(&mut self, line: String, color: LineColor) {
        self.lines.push_back((line, color));
        while self.lines.len() > MAX_LINES {
            self.lines.pop_front();
        }
    }

    /// Splits a trailing `> path` (or `>> path`, treated the same as `>` --
    /// RAMFS has no append primitive) off a command line. Returns the
    /// pipeline part and the redirect target, if any.
    fn split_redirect(line: &str) -> (&str, Option<&str>) {
        if let Some(idx) = line.find('>') {
            let cmd = line[..idx].trim_end();
            let rest = line[idx..].trim_start_matches('>').trim();
            (cmd, if rest.is_empty() { None } else { Some(rest) })
        } else {
            (line, None)
        }
    }

    /// Runs a full command line: splits off any `> file` redirect, then runs
    /// each `|`-separated stage in order, feeding each stage's output lines
    /// into the next as real "stdin" (only `grep`/`sort`/`wc`/`head`/`tail`
    /// actually read it -- everything else ignores it, same as a real shell
    /// pipeline where not every command needs stdin).
    fn run_command(&mut self, line: &str) {
        let (pipeline, redirect) = Self::split_redirect(line.trim());
        let stages: Vec<&str> = pipeline
            .split('|')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();

        let mut output: Vec<(String, LineColor)> = Vec::new();
        for (i, stage) in stages.iter().enumerate() {
            let mut stage_out = Vec::new();
            let stdin = if i == 0 {
                None
            } else {
                Some(output.iter().map(|(s, _)| s.clone()).collect::<Vec<_>>())
            };
            self.dispatch(stage, stdin.as_deref(), &mut stage_out);
            output = stage_out;
        }

        match redirect {
            Some(target) => {
                let path = resolve(&self.cwd, target);
                let data = output
                    .iter()
                    .map(|(s, _)| s.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                crate::fs::root().lock().write(&path, data.as_bytes());
                self.push_line(
                    format!("(redirected {} line(s) to {})", output.len(), path),
                    LineColor::Success,
                );
            }
            None => {
                for (line, color) in output {
                    self.push_line(line, color);
                }
            }
        }
    }

    /// Runs one pipeline stage. `stdin` is `Some` for every stage after the
    /// first -- the previous stage's output lines, verbatim.
    fn dispatch(
        &mut self,
        line: &str,
        stdin: Option<&[String]>,
        out: &mut Vec<(String, LineColor)>,
    ) {
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("help") => out.push((
                String::from(
                    "help clear uptime mem echo pkg pwd cd ls mkdir touch rm cp mv cat whoami date time history reboot shutdown | pipes: grep/sort/wc/head/tail",
                ),
                LineColor::Accent,
            )),
            Some("clear") => self.lines.clear(),
            Some("uptime") => {
                let ticks = crate::sched::ticks();
                out.push((
                    format!("up {} ticks (~{}s at 100Hz)", ticks, ticks / 100),
                    LineColor::Normal,
                ));
            }
            Some("mem") => {
                let stats = crate::memory::pmm::stats();
                out.push((
                    format!(
                        "{} MiB free / {} MiB total",
                        (stats.free_frames * 4096) / (1024 * 1024),
                        (stats.total_frames * 4096) / (1024 * 1024)
                    ),
                    LineColor::Normal,
                ));
            }
            Some("echo") => out.push((parts.collect::<Vec<_>>().join(" "), LineColor::Normal)),
            Some("pkg") => self.run_pkg_command(parts.next(), parts.next(), out),
            Some("pwd") => out.push((self.cwd.clone(), LineColor::Normal)),
            Some("cd") => self.run_cd(parts.next(), out),
            Some("ls") => self.run_ls(parts.next(), out),
            Some("mkdir") => self.run_mkdir(parts.next(), out),
            Some("touch") => self.run_touch(parts.next(), out),
            Some("rm") => self.run_rm(parts.next(), out),
            Some("cp") => self.run_cp(parts.next(), parts.next(), out),
            Some("mv") => self.run_mv(parts.next(), parts.next(), out),
            Some("cat") => self.run_cat(parts.next(), out),
            Some("whoami") => out.push((String::from("moon"), LineColor::Normal)),
            Some("date") => {
                let dt = crate::drivers::rtc::read();
                out.push((
                    format!("{:04}-{:02}-{:02}", dt.year, dt.month, dt.day),
                    LineColor::Normal,
                ));
            }
            Some("time") => {
                let dt = crate::drivers::rtc::read();
                out.push((
                    format!("{:02}:{:02}:{:02}", dt.hour, dt.minute, dt.second),
                    LineColor::Normal,
                ));
            }
            Some("history") => {
                for (i, cmd) in self.history.iter().enumerate() {
                    out.push((format!("{:4}  {}", i + 1, cmd), LineColor::Accent));
                }
            }
            Some("grep") => match (stdin, parts.next()) {
                (Some(lines), Some(pattern)) => {
                    for l in lines {
                        if l.contains(pattern) {
                            out.push((l.clone(), LineColor::Normal));
                        }
                    }
                }
                (None, _) => out.push((
                    String::from("grep: needs piped input, e.g. ls | grep <pattern>"),
                    LineColor::Error,
                )),
                (_, None) => out.push((String::from("usage: <cmd> | grep <pattern>"), LineColor::Error)),
            },
            Some("sort") => match stdin {
                Some(lines) => {
                    let mut sorted: Vec<String> = lines.to_vec();
                    sorted.sort();
                    for l in sorted {
                        out.push((l, LineColor::Normal));
                    }
                }
                None => out.push((
                    String::from("sort: needs piped input, e.g. ls | sort"),
                    LineColor::Error,
                )),
            },
            Some("wc") => match stdin {
                Some(lines) => out.push((format!("{} line(s)", lines.len()), LineColor::Normal)),
                None => out.push((
                    String::from("wc: needs piped input, e.g. ls | wc"),
                    LineColor::Error,
                )),
            },
            Some("head") => match stdin {
                Some(lines) => {
                    let n: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(10);
                    for l in lines.iter().take(n) {
                        out.push((l.clone(), LineColor::Normal));
                    }
                }
                None => out.push((
                    String::from("head: needs piped input, e.g. ls | head 5"),
                    LineColor::Error,
                )),
            },
            Some("tail") => match stdin {
                Some(lines) => {
                    let n: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(10);
                    let start = lines.len().saturating_sub(n);
                    for l in &lines[start..] {
                        out.push((l.clone(), LineColor::Normal));
                    }
                }
                None => out.push((
                    String::from("tail: needs piped input, e.g. ls | tail 5"),
                    LineColor::Error,
                )),
            },
            Some("reboot") => crate::power::reboot(),
            Some("shutdown") => crate::power::shutdown(),
            Some(other) => out.push((
                format!("unknown command: {other} (try 'help')"),
                LineColor::Error,
            )),
            None => {}
        }
    }

    fn run_cd(&mut self, arg: Option<&str>, out: &mut Vec<(String, LineColor)>) {
        let target = resolve(&self.cwd, arg.unwrap_or("/"));
        let root = crate::fs::root().lock();
        if root.is_dir(&target) {
            drop(root);
            self.cwd = target;
        } else {
            out.push((format!("cd: not a directory: {target}"), LineColor::Error));
        }
    }

    fn run_ls(&mut self, arg: Option<&str>, out: &mut Vec<(String, LineColor)>) {
        let target = resolve(&self.cwd, arg.unwrap_or("."));
        let root = crate::fs::root().lock();
        if !root.is_dir(&target) {
            out.push((format!("ls: not a directory: {target}"), LineColor::Error));
            return;
        }
        let entries = root.list_dir(&target);
        if entries.is_empty() {
            out.push((String::from("(empty)"), LineColor::Normal));
        }
        for (path, is_dir) in entries {
            let name = path.rsplit('/').next().unwrap_or(&path);
            if is_dir {
                out.push((format!("{name}/"), LineColor::Dir));
            } else {
                out.push((name.to_string(), LineColor::Normal));
            }
        }
    }

    fn run_mkdir(&mut self, arg: Option<&str>, out: &mut Vec<(String, LineColor)>) {
        match arg {
            Some(path) => {
                let target = resolve(&self.cwd, path);
                crate::fs::root().lock().mkdir(&target);
                out.push((format!("created {target}"), LineColor::Success));
            }
            None => out.push((String::from("usage: mkdir <path>"), LineColor::Error)),
        }
    }

    fn run_touch(&mut self, arg: Option<&str>, out: &mut Vec<(String, LineColor)>) {
        match arg {
            Some(path) => {
                let target = resolve(&self.cwd, path);
                let mut root = crate::fs::root().lock();
                if !root.exists(&target) {
                    root.write(&target, b"");
                }
                out.push((format!("touched {target}"), LineColor::Success));
            }
            None => out.push((String::from("usage: touch <path>"), LineColor::Error)),
        }
    }

    fn run_rm(&mut self, arg: Option<&str>, out: &mut Vec<(String, LineColor)>) {
        match arg {
            Some(path) => {
                let target = resolve(&self.cwd, path);
                let mut root = crate::fs::root().lock();
                if root.remove(&target) || root.rmdir(&target) {
                    out.push((format!("removed {target}"), LineColor::Success));
                } else {
                    out.push((format!("rm: no such file: {target}"), LineColor::Error));
                }
            }
            None => out.push((String::from("usage: rm <path>"), LineColor::Error)),
        }
    }

    fn run_cp(&mut self, src: Option<&str>, dst: Option<&str>, out: &mut Vec<(String, LineColor)>) {
        match (src, dst) {
            (Some(src), Some(dst)) => {
                let (src, dst) = (resolve(&self.cwd, src), resolve(&self.cwd, dst));
                if crate::fs::root().lock().copy(&src, &dst) {
                    out.push((format!("copied {src} -> {dst}"), LineColor::Success));
                } else {
                    out.push((
                        format!("cp: failed ({src} missing, or {dst} exists?)"),
                        LineColor::Error,
                    ));
                }
            }
            _ => out.push((String::from("usage: cp <src> <dst>"), LineColor::Error)),
        }
    }

    fn run_mv(&mut self, src: Option<&str>, dst: Option<&str>, out: &mut Vec<(String, LineColor)>) {
        match (src, dst) {
            (Some(src), Some(dst)) => {
                let (src, dst) = (resolve(&self.cwd, src), resolve(&self.cwd, dst));
                if crate::fs::root().lock().rename(&src, &dst) {
                    out.push((format!("moved {src} -> {dst}"), LineColor::Success));
                } else {
                    out.push((
                        format!("mv: failed ({src} missing, or {dst} exists?)"),
                        LineColor::Error,
                    ));
                }
            }
            _ => out.push((String::from("usage: mv <src> <dst>"), LineColor::Error)),
        }
    }

    fn run_cat(&mut self, arg: Option<&str>, out: &mut Vec<(String, LineColor)>) {
        match arg {
            Some(path) => {
                let target = resolve(&self.cwd, path);
                let root = crate::fs::root().lock();
                match root.read(&target) {
                    Some(data) => match core::str::from_utf8(data) {
                        Ok(text) => {
                            for line in text.lines() {
                                out.push((line.to_string(), LineColor::Normal));
                            }
                        }
                        Err(_) => out.push((
                            format!("cat: {target} is not valid UTF-8"),
                            LineColor::Error,
                        )),
                    },
                    None => out.push((format!("cat: no such file: {target}"), LineColor::Error)),
                }
            }
            None => out.push((String::from("usage: cat <path>"), LineColor::Error)),
        }
    }

    fn run_pkg_command(
        &mut self,
        sub: Option<&str>,
        arg: Option<&str>,
        out: &mut Vec<(String, LineColor)>,
    ) {
        match sub {
            Some("list") => {
                let packages = crate::pkg::installed();
                if packages.is_empty() {
                    out.push((String::from("no packages installed"), LineColor::Normal));
                }
                for p in packages {
                    out.push((
                        format!("{} {} ({})", p.name, p.version, p.file_name),
                        LineColor::Normal,
                    ));
                }
            }
            Some("run") => match arg {
                Some(name) => {
                    let target = crate::pkg::installed()
                        .into_iter()
                        .find(|p| p.name == name)
                        .map(|p| p.file_name);
                    match target {
                        Some(file_name) => match crate::pkg::run(&file_name) {
                            Ok(name) => out.push((format!("running {name}"), LineColor::Success)),
                            Err(err) => {
                                out.push((format!("pkg run failed: {err}"), LineColor::Error))
                            }
                        },
                        None => out.push((format!("no such package: {name}"), LineColor::Error)),
                    }
                }
                None => out.push((String::from("usage: pkg run <name>"), LineColor::Error)),
            },
            Some(other) => out.push((format!("unknown pkg subcommand: {other}"), LineColor::Error)),
            None => out.push((
                String::from("usage: pkg list | pkg run <name>"),
                LineColor::Error,
            )),
        }
    }

    pub fn render(&self, x: i32, y: i32, w: u32, h: u32) {
        framebuffer::with(|c| {
            c.fill_rect(x, y, w, h, (0x08, 0x08, 0x0C));

            let visible_rows = ((h as i32 - 4) / LINE_HEIGHT).max(1) as usize;
            let input_row = visible_rows.saturating_sub(1);
            let history_rows = input_row;
            let start = self.lines.len().saturating_sub(history_rows);

            let max_chars = ((w as i32 - 8) / 8).max(1) as usize;
            for (row, (line, color)) in self.lines.iter().skip(start).enumerate() {
                let text = if line.len() > max_chars {
                    &line[..max_chars]
                } else {
                    line.as_str()
                };
                c.draw_str_at(
                    x + 4,
                    y + 4 + row as i32 * LINE_HEIGHT,
                    text,
                    color.rgb(),
                    None,
                );
            }

            let prompt = format!("{} > {}_", self.cwd, self.current);
            let prompt = if prompt.len() > max_chars {
                &prompt[prompt.len() - max_chars..]
            } else {
                prompt.as_str()
            };
            c.draw_str_at(
                x + 4,
                y + 4 + input_row as i32 * LINE_HEIGHT,
                prompt,
                (0x40, 0xFF, 0x40),
                None,
            );
        });
    }
}

impl Default for TerminalState {
    fn default() -> Self {
        Self::new()
    }
}
