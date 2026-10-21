//! Screen rendering and key handling of the full-screen config editor.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use crossterm::terminal;

use crate::clipboard;
use crate::config_diagnostics::editor_completions;
use crate::config_template::{add_config_guide, remove_config_guide};
use crate::error::{Error, Result};
use crate::path_links::folder_link;

use super::*;

pub(super) struct Editor<'a> {
    pub(super) file: &'a Path,
    pub(super) color: bool,
    pub(super) newline: &'static str,
    pub(super) buffer: EditorBuffer,
    pub(super) history: History,
    pub(super) saved: Vec<char>,
    pub(super) disk: String,
    pub(super) issue: Option<EditorIssue>,
    pub(super) top: usize,
    pub(super) left: usize,
    pub(super) follow: bool,
    pub(super) question: bool,
    pub(super) guide_prompt: bool,
    pub(super) status: String,
    pub(super) completion: Option<(usize, usize, Vec<serde_json::Value>, usize)>,
    pub(super) pending: Option<Instant>,
    pub(super) viewport: (usize, usize, usize),
    pub(super) dragging: bool,
}

impl Editor<'_> {
    pub(super) fn validate(&mut self) {
        self.issue = validate_editor_text(&self.buffer.string());
        self.pending = None;
    }

    pub(super) fn render(&mut self) {
        let (columns, rows) = terminal::size().map(|(c, r)| (c as usize, r as usize)).unwrap_or((80, 24));
        let width = columns.saturating_sub(1).max(1);
        let diagnosis = self
            .issue
            .as_ref()
            .map(|issue| {
                format!("{}{}", issue.line.map(|line| format!("Line {line}, column {}: ", issue.column.unwrap_or(1))).unwrap_or_default(), issue.message)
            })
            .unwrap_or_default();
        let detail = safe(&match &self.completion {
            Some((_, _, choices, index)) => format!("Options ({}/{}): {} | Up/Down choose, Enter apply, Esc cancel", index + 1, choices.len(), choices[*index]),
            None if !self.status.is_empty() => format!("{}{}", self.status, if diagnosis.is_empty() { String::new() } else { format!(" | {diagnosis}") }),
            None if !diagnosis.is_empty() => diagnosis,
            None => "Config OK".into(),
        });
        let help: [&str; 2] = if self.question {
            ["Discard unsaved changes? Y = discard, N / Esc = keep editing", ""]
        } else if self.guide_prompt {
            ["Guide: A add/update | R remove | Esc cancel", ""]
        } else {
            ["F3 Guide | F2 Options | Ctrl+S Save | Ctrl+Z Undo | Ctrl+Y Redo", "Ctrl+A All | Ctrl+C Copy | Ctrl+V Paste | Esc / Ctrl+Q Exit"]
        };
        let height = rows.saturating_sub(3).max(1);
        let text = self.buffer.string();
        let lines: Vec<&str> = text.split('\n').collect();
        let (row, col) = self.buffer.position();
        let gutter = (lines.len().to_string().len() + 2).min(width.saturating_sub(1));
        let available = width.saturating_sub(gutter).max(1);
        if self.follow {
            self.top = self.top.min(row);
            if row >= self.top + height {
                self.top = row + 1 - height;
            }
        }
        self.top = self.top.min(lines.len().saturating_sub(height));
        self.left = self.left.min(col);
        if col >= self.left + available {
            self.left = col + 1 - available;
        }
        self.viewport = (height, gutter, width);
        let (selection_start, selection_end) = self.buffer.selection();
        let mut offset: usize = lines[..self.top].iter().map(|line| line.chars().count() + 1).sum();
        let modified = if self.buffer.text != self.saved { " *" } else { "" };
        let shown = safe(&self.file.display().to_string());
        let heading: String = format!("veo config | {shown}{modified}").chars().take(width).collect();
        let label: String = heading.chars().skip(13).take(shown.chars().count()).collect();
        let mut screen = vec![format!(
            "{}{}{}",
            heading.chars().take(13).collect::<String>(),
            folder_link(true, self.file, &label),
            heading.chars().skip(13 + label.chars().count()).collect::<String>()
        )];
        for index in self.top..self.top + height {
            let line = lines.get(index).copied();
            let prefix = match line {
                Some(_) => format!(
                    "{}{:>width$} ",
                    if self.issue.as_ref().and_then(|issue| issue.line) == Some(index + 1) { "!" } else { " " },
                    index + 1,
                    width = gutter.saturating_sub(2)
                ),
                None => String::new(),
            };
            let chars: Vec<char> = line.unwrap_or("").chars().collect();
            let content: String = chars.iter().skip(self.left).take(available).collect();
            let length = content.chars().count();
            let start = selection_start.saturating_sub(offset + self.left);
            let end = selection_end.saturating_sub(offset + self.left).min(length);
            let slice = |from: usize, to: usize| content.chars().skip(from).take(to.saturating_sub(from)).collect::<String>();
            if end > start && selection_end > offset + self.left {
                screen.push(format!(
                    "{prefix}{}\x1b[7m{}\x1b[0m{}",
                    color_line(&slice(0, start), self.color),
                    safe(&slice(start, end)),
                    color_line(&slice(end, length), self.color)
                ));
            } else if let (Some(issue_start), Some(issue_end), true) =
                (self.issue.as_ref().and_then(|issue| issue.start), self.issue.as_ref().and_then(|issue| issue.end), self.color)
            {
                let from = issue_start.saturating_sub(offset + self.left);
                let to = issue_end.saturating_sub(offset + self.left).min(length);
                if to > from && issue_end > offset + self.left {
                    screen.push(format!(
                        "{prefix}{}\x1b[41;97m{}\x1b[0m{}",
                        color_line(&slice(0, from), self.color),
                        safe(&slice(from, to)),
                        color_line(&slice(to, length), self.color)
                    ));
                } else {
                    screen.push(format!("{prefix}{}", color_line(&content, self.color)));
                }
            } else if self.issue.as_ref().and_then(|issue| issue.line) == Some(index + 1) && self.color {
                screen.push(format!("\x1b[41;97m{prefix}{}\x1b[0m", safe(&content)));
            } else {
                screen.push(format!("{prefix}{}", color_line(&content, self.color)));
            }
            offset += chars.len() + 1;
        }
        let position = format!("Ln {}, Col {}", row + 1, col + 1);
        let tone = if self.completion.is_some() {
            33
        } else if self.issue.is_some() {
            31
        } else if !self.status.is_empty() {
            32
        } else {
            90
        };
        let color = self.color;
        let styled_detail = |value: &str| if color { format!("\x1b[{tone}m{value}\x1b[0m") } else { value.to_string() };
        let styled_help =
            |value: &str| if color { re!(r"Ctrl\+[A-Z]|F[23]|Esc|\b[AR]\b").replace_all(value, "\x1b[1;36m$0\x1b[0m").into_owned() } else { value.to_string() };
        let prompt = self.question || self.guide_prompt;
        let long_detail = prompt || detail.chars().count() + position.len() + 3 > (width / 2).max(30);
        let first_help: String = safe(help[0]).chars().take(width).collect();
        screen.push(if prompt {
            styled_help(&first_help)
        } else if long_detail {
            styled_detail(&detail.chars().take(width).collect::<String>())
        } else {
            styled_help(&first_help)
        });
        let right = format!("{}{position}", if long_detail { String::new() } else { format!("{detail} | ") });
        let help_width = width.saturating_sub(right.chars().count() + 1);
        let second: String = safe(help[1]).chars().take(help_width).collect();
        let padding = " ".repeat(width.saturating_sub(second.chars().count() + right.chars().count()));
        screen.push(format!(
            "{}{padding}{}",
            styled_help(&second),
            if long_detail { position.clone() } else { format!("{} | {position}", styled_detail(&detail)) }
        ));
        let visible = row >= self.top && row < self.top + height;
        let cursor = if visible { format!("\x1b[{};{}H\x1b[?25h", row - self.top + 2, gutter + col - self.left + 1) } else { String::new() };
        let frame = format!("\x1b[?25l\x1b[H{}{cursor}", screen.iter().map(|line| format!("{line}\x1b[K")).collect::<Vec<_>>().join("\r\n"));
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(frame.as_bytes());
        let _ = out.flush();
    }

    pub(super) fn edited(&mut self, before: Snapshot, kind: &str, groupable: bool) {
        if before.text != self.buffer.text {
            let buffer = self.buffer.clone();
            self.history.record(&buffer, before, kind, groupable);
            self.status.clear();
            self.issue = None;
            self.pending = Some(Instant::now());
        } else {
            self.history.group = None;
        }
    }

    pub(super) fn save(&mut self) {
        self.history.group = None;
        self.follow = true;
        self.buffer.anchor = None;
        self.validate();
        if let Some(issue) = &self.issue {
            if let Some(line) = issue.line {
                let text = self.buffer.string();
                let offset: usize = text.split('\n').take(line - 1).map(|line| line.chars().count() + 1).sum();
                self.buffer.cursor = (offset + issue.column.unwrap_or(1) - 1).min(self.buffer.text.len());
            }
            return;
        }
        let result = (|| -> Result<()> {
            if String::from_utf8_lossy(&fs::read(self.file)?) != self.disk {
                return Err(Error::msg("File changed outside the editor. Exit and reopen before saving."));
            }
            let next = self.buffer.string().replace('\n', self.newline);
            fs::write(self.file, &next)?;
            self.disk = next;
            self.saved = self.buffer.text.clone();
            Ok(())
        })();
        self.status = match result {
            Ok(()) => "Saved".into(),
            Err(error) => format!("Save failed: {}", error.message),
        };
    }

    /// Returns true when the editor should close.
    pub(super) fn key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let text = match key.code {
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => Some(c.to_string()),
            _ => None,
        };
        let lower = text.as_deref().map(str::to_lowercase);
        if self.dragging {
            self.dragging = false;
        }
        if self.question {
            if lower.as_deref() == Some("y") {
                return true;
            }
            if lower.as_deref() == Some("n") || key.code == KeyCode::Esc {
                self.question = false;
            }
            return false;
        }
        if self.guide_prompt {
            self.guide_prompt = false;
            if let Some(choice @ ("a" | "r")) = lower.as_deref() {
                let before = History::snapshot(&self.buffer);
                let current = self.buffer.string();
                let updated = if choice == "a" { add_config_guide(&current) } else { remove_config_guide(&current) };
                if updated != current {
                    self.buffer = EditorBuffer::new(&updated);
                    self.follow = true;
                    let buffer = self.buffer.clone();
                    self.history.record(&buffer, before, "guide", false);
                    self.validate();
                    self.status = if choice == "a" { "Guide added/updated. Ctrl+S saves it." } else { "Generated guide removed. Ctrl+S saves it." }.into();
                } else {
                    self.status = if choice == "r" { "No generated guide to remove." } else { "Guide is already current." }.into();
                }
            }
            return false;
        }
        match key.code {
            KeyCode::Char('a') if ctrl => {
                self.history.group = None;
                self.completion = None;
                self.buffer.anchor = Some(0);
                self.buffer.cursor = self.buffer.text.len();
                self.follow = true;
                self.status = "All text selected.".into();
                return false;
            }
            KeyCode::Char(c @ ('z' | 'y')) if ctrl => {
                self.completion = None;
                let undo = c == 'z';
                if self.history.change(&mut self.buffer, undo) {
                    self.follow = true;
                    self.validate();
                    self.status = if undo { "Undone." } else { "Redone." }.into();
                } else {
                    self.status = if undo { "Nothing to undo." } else { "Nothing to redo." }.into();
                }
                return false;
            }
            _ => {}
        }
        if let Some((start, end, choices, index)) = &mut self.completion {
            match key.code {
                KeyCode::Esc => self.completion = None,
                KeyCode::Up => *index = (*index + choices.len() - 1) % choices.len(),
                KeyCode::Down | KeyCode::Tab => *index = (*index + 1) % choices.len(),
                KeyCode::Enter => {
                    let before = History::snapshot(&self.buffer);
                    let replacement = choices[*index].to_string();
                    let text = self.buffer.string();
                    self.buffer.anchor = Some(char_offset(&text, *start));
                    self.buffer.cursor = char_offset(&text, *end);
                    self.buffer.insert(&replacement);
                    let buffer = self.buffer.clone();
                    self.history.record(&buffer, before, "completion", false);
                    self.completion = None;
                    self.status.clear();
                    self.validate();
                }
                _ => {}
            }
            return false;
        }
        match key.code {
            KeyCode::F(2) | KeyCode::Char(' ') if key.code == KeyCode::F(2) || ctrl => {
                let text = self.buffer.string();
                let byte = text.char_indices().nth(self.buffer.cursor).map_or(text.len(), |(index, _)| index);
                match editor_completions(&text, byte) {
                    Some(options) => {
                        self.completion = Some((options.start, options.end, options.choices, 0));
                        self.status.clear();
                    }
                    None => {
                        self.completion = None;
                        self.status = "No options here. Place the cursor on a property or value in valid JSON.".into();
                    }
                }
                return false;
            }
            KeyCode::F(3) => {
                self.completion = None;
                self.guide_prompt = true;
                return false;
            }
            KeyCode::Char('c') if ctrl => {
                let (start, end) = self.buffer.selection();
                self.status = if start == end {
                    "Select text first, then press Ctrl+C.".into()
                } else {
                    match clipboard::copy(&self.buffer.text[start..end].iter().collect::<String>()) {
                        Ok(()) => "Copied to clipboard.".into(),
                        Err(error) => format!("Copy failed: {}", error.message),
                    }
                };
                return false;
            }
            KeyCode::Char('v') if ctrl => {
                match clipboard::paste() {
                    Ok(pasted) => self.paste(&pasted),
                    Err(error) => self.status = format!("Paste failed: {}", error.message),
                }
                return false;
            }
            KeyCode::Up | KeyCode::Down if ctrl => {
                self.follow = false;
                self.top = if key.code == KeyCode::Up { self.top.saturating_sub(3) } else { self.top + 3 };
                return false;
            }
            KeyCode::Esc => return self.request_exit(),
            KeyCode::Char('q') if ctrl => return self.request_exit(),
            KeyCode::Char('s') if ctrl => {
                self.save();
                return false;
            }
            _ => {}
        }
        if ctrl || key.modifiers.contains(KeyModifiers::ALT) {
            return false;
        }
        self.follow = true;
        let before = History::snapshot(&self.buffer);
        let had_selection = self.buffer.selection().0 != self.buffer.selection().1;
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let name = match key.code {
            KeyCode::Left => "left",
            KeyCode::Right => "right",
            KeyCode::Up => "up",
            KeyCode::Down => "down",
            KeyCode::Home => "home",
            KeyCode::End => "end",
            KeyCode::Backspace => "backspace",
            KeyCode::Delete => "delete",
            KeyCode::Enter => "return",
            KeyCode::Tab => "tab",
            KeyCode::PageUp | KeyCode::PageDown => {
                let rows = self.viewport.0 as isize;
                self.buffer.move_rows(if key.code == KeyCode::PageUp { -rows } else { rows });
                ""
            }
            _ => "",
        };
        if !name.is_empty() {
            let anchor = self.buffer.anchor.or(Some(self.buffer.cursor));
            self.buffer.key(name);
            if shift && ["left", "right", "up", "down", "home", "end"].contains(&name) {
                self.buffer.anchor = anchor;
            }
        } else if let Some(text) = &text {
            self.buffer.insert(text);
        }
        let typing = text.is_some() && !had_selection;
        let deleting = (name == "backspace" || name == "delete") && !had_selection;
        let kind = if typing {
            "typing"
        } else if deleting {
            name
        } else if name.is_empty() {
            "edit"
        } else {
            name
        };
        self.edited(before, kind, typing || deleting);
        false
    }

    pub(super) fn paste(&mut self, pasted: &str) {
        let normalized = re!(r"\r\n?").replace_all(pasted, "\n");
        let cleaned: String = normalized.chars().filter(|c| !matches!(c, '\u{00}'..='\u{08}' | '\u{0b}'..='\u{1f}' | '\u{7f}'..='\u{9f}')).collect();
        if cleaned.is_empty() {
            self.status = "Clipboard has no text.".into();
            return;
        }
        let before = History::snapshot(&self.buffer);
        self.buffer.insert(&cleaned);
        self.follow = true;
        let buffer = self.buffer.clone();
        self.history.record(&buffer, before, "paste", false);
        self.status.clear();
        self.validate();
    }

    pub(super) fn request_exit(&mut self) -> bool {
        if self.buffer.text == self.saved {
            return true;
        }
        self.question = true;
        false
    }

    pub(super) fn mouse(&mut self, kind: MouseEventKind, x: usize, y: usize) {
        if self.question {
            return;
        }
        let (height, gutter, width) = self.viewport;
        match kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                self.follow = false;
                self.top = if kind == MouseEventKind::ScrollUp { self.top.saturating_sub(3) } else { self.top + 3 };
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.completion = None;
                if y < 2 || y > height + 1 || x < 1 || x > width {
                    return;
                }
                self.dragging = true;
                self.history.group = None;
                self.follow = true;
                self.buffer.point((self.top + y - 2) as isize, (self.left + x) as isize - gutter as isize - 1, false);
                self.buffer.anchor = Some(self.buffer.cursor);
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                let row = y.saturating_sub(2).min(height.saturating_sub(1));
                if y <= 2 {
                    self.top = self.top.saturating_sub(1);
                } else if y > height {
                    self.top += 1;
                }
                self.follow = false;
                self.buffer.point((self.top + row) as isize, (self.left + x) as isize - gutter as isize - 1, true);
            }
            MouseEventKind::Up(_) => self.dragging = false,
            _ => {}
        }
    }
}
