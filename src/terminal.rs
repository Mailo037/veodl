use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::utils::clean_text;

/// One of the two standard output streams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Std {
    Out,
    Err,
}

impl Std {
    pub fn is_tty(self) -> bool {
        match self {
            Std::Out => std::io::stdout().is_terminal(),
            Std::Err => std::io::stderr().is_terminal(),
        }
    }

    pub fn write(self, text: &str) {
        // Broken pipes (e.g. `veo … | head`) must not abort the program.
        let _ = match self {
            Std::Out => {
                let mut out = std::io::stdout().lock();
                out.write_all(text.as_bytes()).and_then(|_| out.flush())
            }
            Std::Err => {
                let mut err = std::io::stderr().lock();
                err.write_all(text.as_bytes()).and_then(|_| err.flush())
            }
        };
    }
}

/// Terminal width in cells; 80 when unknown.
pub fn columns() -> usize {
    match crossterm::terminal::size() {
        Ok((width, _)) if width > 0 => width as usize,
        _ => 80,
    }
}

pub fn rows() -> usize {
    match crossterm::terminal::size() {
        Ok((_, height)) if height > 0 => height as usize,
        _ => 24,
    }
}

pub fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal()
}

fn env_set(name: &str) -> bool {
    std::env::var_os(name).is_some()
}

pub fn dumb() -> bool {
    std::env::var("TERM").is_ok_and(|term| term == "dumb")
}

/// Process-wide color switch, set from --color/--no-color, --json and profiles.
static COLOR: AtomicBool = AtomicBool::new(true);

pub fn set_color(enabled: bool) {
    COLOR.store(enabled, Ordering::SeqCst);
}

pub fn color_enabled() -> bool {
    COLOR.load(Ordering::SeqCst)
}

/// Apply an SGR role when the stream is a capable terminal and color is wanted.
pub fn style(tty: bool, text: &str, role: &str, enabled: bool) -> String {
    if !enabled || !tty || env_set("NO_COLOR") || dumb() {
        return text.to_string();
    }
    let code = match role {
        "title" => 1,
        "profile" => 97,
        "success" => 32,
        "warn" => 33,
        "error" => 31,
        "accent" => 36,
        _ => 90,
    };
    format!("\x1b[{code}m{text}\x1b[0m")
}

fn wide(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x115f | 0x2329 | 0x232a | 0x2e80..=0xa4cf | 0xac00..=0xd7a3 | 0xf900..=0xfaff | 0xfe10..=0xfe19
        | 0xfe30..=0xfe6f | 0xff01..=0xff60 | 0xffe0..=0xffe6)
        || re!(r"^\p{Extended_Pictographic}$").is_match(c.encode_utf8(&mut [0; 4]))
}

fn zero_width(c: char) -> bool {
    c == '\u{200d}' || re!(r"^\p{M}$").is_match(c.encode_utf8(&mut [0; 4]))
}

pub fn char_cells(c: char) -> usize {
    if (c as u32) < 0x300 {
        return 1;
    }
    if zero_width(c) {
        0
    } else if wide(c) {
        2
    } else {
        1
    }
}

/// Count terminal cells conservatively, including wide titles and emoji.
pub fn cells(text: &str) -> usize {
    text.chars().map(char_cells).sum()
}

/// Shorten to `width` cells with a trailing ellipsis.
pub fn fit(text: &str, width: usize) -> String {
    if cells(text) <= width {
        return text.to_string();
    }
    let mut result = String::new();
    let mut used = 0;
    for c in text.chars() {
        let size = char_cells(c);
        if used + size > width.saturating_sub(1) {
            break;
        }
        used += size;
        result.push(c);
    }
    if width > 0 {
        result.push('…');
    }
    result
}

/// Hard-wrap at `width` cells; `None` means unlimited.
pub fn wrap(text: &str, width: Option<usize>) -> String {
    let Some(width) = width else { return text.to_string() };
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for c in text.chars() {
        let size = char_cells(c);
        if !line.is_empty() && used + size > width {
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        line.push(c);
        used += size;
    }
    lines.push(line);
    lines.join("\n")
}

/// Break at spaces; only a word wider than the line is split.
pub fn word_wrap(text: &str, width: Option<usize>) -> String {
    let Some(limit) = width else { return text.to_string() };
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in re!(r" +").split(text) {
        let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if cells(&candidate) <= limit {
            line = candidate;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        let mut pieces: Vec<String> = wrap(word, Some(limit)).split('\n').map(str::to_string).collect();
        line = pieces.pop().unwrap_or_default();
        lines.extend(pieces);
    }
    lines.push(line);
    lines.join("\n")
}

/// Width available for one line of text on a stream, or None when redirected.
pub fn line_width(tty: bool) -> Option<usize> {
    tty.then(|| columns().saturating_sub(1).max(1))
}

pub fn terminal_text(tty: bool, text: &str) -> String {
    let clean = clean_text(text);
    match line_width(tty) {
        Some(width) => fit(&clean, width),
        None => clean,
    }
}

/// Wrap a title over at most three rows before shortening it.
pub fn terminal_title(tty: bool, text: &str) -> String {
    terminal_title_width(line_width(tty), text)
}

pub fn terminal_title_width(width: Option<usize>, text: &str) -> String {
    let safe = clean_text(text);
    let Some(width) = width else { return safe };
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    let chars: Vec<char> = safe.chars().collect();
    for (index, c) in chars.iter().enumerate() {
        let candidate = format!("{line}{c}");
        if cells(&candidate) > width {
            if lines.len() == 2 {
                let _ = index;
                lines.push(fit(&format!("{line}{c}…"), width));
                return lines.join("\n");
            }
            lines.push(std::mem::take(&mut line));
        }
        line.push(*c);
    }
    lines.push(line);
    lines.join("\n")
}

/// Rows for a labelled path, wrapped (preferably after a separator) instead of
/// shortened: a saved file name that loses its tail cannot be recovered.
pub fn path_rows(tty: bool, label: &str, value: &str) -> Vec<String> {
    let safe = clean_text(value);
    let width = if tty { Some(columns().saturating_sub(1 + cells(label)).max(1)) } else { None };
    let Some(width) = width.filter(|width| *width >= 8 && cells(&safe) > *width) else { return vec![safe] };
    let mut rows = Vec::new();
    let mut rest: Vec<char> = safe.chars().collect();
    while cells(&rest.iter().collect::<String>()) > width {
        let mut piece: Vec<char> = Vec::new();
        let mut used = 0;
        for c in &rest {
            let size = char_cells(*c);
            if used + size > width {
                break;
            }
            used += size;
            piece.push(*c);
        }
        if let Some(separator) = piece.iter().rposition(|c| *c == '/' || *c == '\\') {
            if separator as f64 >= width as f64 / 3.0 {
                piece.truncate(separator + 1);
            }
        }
        if piece.is_empty() {
            break;
        }
        rest.drain(..piece.len());
        rows.push(piece.into_iter().collect());
    }
    if !rest.is_empty() {
        rows.push(rest.into_iter().collect());
    }
    rows
}

/// Block glyphs need a terminal that understands Unicode; C/POSIX locales and
/// dumb terminals keep the ASCII bar.
pub fn supports_unicode(tty: bool) -> bool {
    if !tty || dumb() {
        return false;
    }
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"].iter().find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty())).unwrap_or_default();
    !locale.eq_ignore_ascii_case("C") && !locale.eq_ignore_ascii_case("POSIX")
}

/// Ask one question on a terminal and read the answer line. Ctrl+C (which
/// cancels the global token) ends the wait with a cancellation error.
pub fn ask(prompt: &str, output: Std) -> crate::error::Result<String> {
    output.write(prompt);
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = std::io::stdin().read_line(&mut line).map(|read| (read, line));
        let _ = sender.send(result);
    });
    let cancel = crate::cancel::Cancel::global();
    loop {
        cancel.check()?;
        match receiver.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(Ok((0, _))) => return Err(crate::error::Error::cancelled()),
            Ok(Ok((_, line))) => return Ok(line.trim_end_matches(['\r', '\n']).to_string()),
            Ok(Err(error)) => return Err(error.into()),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err(crate::error::Error::cancelled()),
        }
    }
}

/// Set the terminal window title (OSC 0, or the console API on Windows).
pub struct TitleSetter {
    previous: Mutex<String>,
}

impl TitleSetter {
    pub const fn new() -> Self {
        TitleSetter { previous: Mutex::new(String::new()) }
    }

    pub fn set(&self, tty: bool, title: &str) {
        if !tty || dumb() {
            return;
        }
        let safe: String = clean_text(title).chars().take(240).collect();
        let mut previous = self.previous.lock().unwrap_or_else(|poison| poison.into_inner());
        if *previous == safe {
            return;
        }
        #[cfg(windows)]
        {
            let wide: Vec<u16> = safe.encode_utf16().chain(Some(0)).collect();
            // SAFETY: a NUL-terminated UTF-16 buffer that outlives the call.
            unsafe { windows_sys::Win32::System::Console::SetConsoleTitleW(wide.as_ptr()) };
        }
        #[cfg(not(windows))]
        Std::Err.write(&format!("\x1b]0;{safe}\x07"));
        *previous = safe;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_wide_cells() {
        assert_eq!(cells("abc"), 3);
        assert_eq!(cells("日本"), 4);
        assert_eq!(cells("e\u{301}"), 1);
        assert_eq!(cells("😀"), 2);
    }

    #[test]
    fn fits_and_wraps() {
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(wrap("abcdef", Some(4)), "abcd\nef");
        assert_eq!(word_wrap("one two three", Some(7)), "one two\nthree");
        assert_eq!(word_wrap("abcdefghij", Some(4)), "abcd\nefgh\nij");
        assert_eq!(terminal_title_width(Some(4), "abcdefghijklmnop"), "abcd\nefgh\nijk…");
    }
}
