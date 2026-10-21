//! The interactive full-screen editor for the config file.

mod buffer;
mod editor;

use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal;

use crate::config_template::config_guide_state;
use crate::error::{Error, Result};

pub use buffer::*;
use editor::*;

/// The interactive full-screen config editor (alternate screen, raw mode).
pub fn edit_config(file: &Path, color: bool) -> Result<()> {
    if !crate::terminal::stdin_is_tty() || !crate::terminal::Std::Out.is_tty() {
        return Err(Error::msg("The config editor requires an interactive terminal. Set EDITOR to use an external editor."));
    }
    let original = String::from_utf8_lossy(&fs::read(file)?).into_owned();
    let newline = if original.contains("\r\n") { "\r\n" } else { "\n" };
    let text = original.replace("\r\n", "\n");
    let state = config_guide_state(&text);
    let mut editor = Editor {
        file,
        color,
        newline,
        buffer: EditorBuffer::new(&text),
        history: History { undo: Vec::new(), redo: Vec::new(), group: None },
        saved: text.chars().collect(),
        disk: original,
        issue: None,
        top: 0,
        left: 0,
        follow: true,
        question: false,
        guide_prompt: false,
        status: match state {
            "outdated" => "Updated config guide available. Press F3 to choose.".into(),
            "current" => "Generated guide in file. Press F3 to remove it if duplicated.".into(),
            _ => String::new(),
        },
        completion: None,
        pending: None,
        viewport: (1, 0, 80),
        dragging: false,
    };
    editor.validate();
    terminal::enable_raw_mode()?;
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x1b[?1049h");
    let _ = crossterm::execute!(out, crossterm::event::EnableMouseCapture, crossterm::event::EnableBracketedPaste);
    let result = (|| -> Result<()> {
        editor.render();
        loop {
            let wait = match editor.pending {
                Some(at) => Duration::from_millis(150).saturating_sub(at.elapsed()),
                None => Duration::from_millis(500),
            };
            if !event::poll(wait)? {
                if editor.pending.is_some_and(|at| at.elapsed() >= Duration::from_millis(150)) {
                    editor.validate();
                    editor.render();
                }
                continue;
            }
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    if editor.key(key) {
                        return Ok(());
                    }
                }
                Event::Mouse(mouse) => editor.mouse(mouse.kind, mouse.column as usize + 1, mouse.row as usize + 1),
                Event::Paste(text) => editor.paste(&text),
                Event::Resize(_, _) => {}
                _ => continue,
            }
            editor.render();
        }
    })();
    let _ = crossterm::execute!(out, crossterm::event::DisableMouseCapture, crossterm::event::DisableBracketedPaste);
    let _ = out.write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l");
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_edits_lines_and_selections() {
        let mut buffer = EditorBuffer::new("ab\ncd");
        buffer.point(1, 1, false);
        assert_eq!(buffer.position(), (1, 1));
        buffer.key("up");
        assert_eq!(buffer.cursor, 1);
        buffer.key("end");
        buffer.insert("!");
        assert_eq!(buffer.string(), "ab!\ncd");
        buffer.point(0, 0, false);
        buffer.point(1, 2, true);
        buffer.key("backspace");
        assert_eq!(buffer.string(), "");
    }

    #[test]
    fn validates_editor_text() {
        assert!(validate_editor_text("{\"quality\": \"720p\"}").is_none());
        let issue = validate_editor_text("{\n  \"quality\": 7,\n}").unwrap();
        assert_eq!(issue.line, Some(3));
        assert!(color_line("\"a\": true // x", true).contains("\x1b[36m"));
    }
}
