use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use crossterm::terminal;

use crate::clipboard;
use crate::config::parse_config_text;
use crate::config_diagnostics::{editor_completions, semantic_issue};
use crate::config_template::{add_config_guide, config_guide_state, remove_config_guide};
use crate::error::{Error, Result};
use crate::path_links::folder_link;

fn safe(text: &str) -> String {
    text.chars().map(|c| if matches!(c, '\u{00}'..='\u{1f}' | '\u{7f}'..='\u{9f}') { ' ' } else { c }).collect()
}

/// A located problem for display (offsets in characters).
#[derive(Clone, Debug)]
pub struct EditorIssue {
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub start: Option<usize>,
    pub end: Option<usize>,
}

fn char_offset(text: &str, byte: usize) -> usize {
    text.get(..byte.min(text.len())).map_or(text.chars().count(), |prefix| prefix.chars().count())
}

pub fn validate_editor_text(text: &str) -> Option<EditorIssue> {
    match semantic_issue(text) {
        Ok(None) => None,
        Ok(Some(issue)) => Some(EditorIssue {
            message: issue.message,
            line: issue.line,
            column: issue.column,
            start: issue.start.map(|byte| char_offset(text, byte)),
            end: issue.end.map(|byte| char_offset(text, byte)),
        }),
        Err(_) => {
            let error = parse_config_text(text, "config").err().unwrap_or_else(|| Error::msg("Invalid JSON syntax."));
            let location = re!(r"line ([0-9]+), column ([0-9]+)")
                .captures(&error.message)
                .map(|captures| (captures[1].parse().unwrap_or(1), captures[2].parse().unwrap_or(1)));
            let message = re!(r"^The veo config file is not valid JSON: .*?\(line [0-9]+, column [0-9]+\)\. ").replace(&error.message, "").into_owned();
            Some(EditorIssue { message, line: location.map(|(line, _)| line), column: location.map(|(_, column)| column), start: None, end: None })
        }
    }
}

/// A small text buffer with a cursor and an optional selection anchor.
#[derive(Clone, Debug)]
pub struct EditorBuffer {
    pub text: Vec<char>,
    pub cursor: usize,
    pub anchor: Option<usize>,
}

impl EditorBuffer {
    pub fn new(text: &str) -> Self {
        EditorBuffer { text: text.chars().collect(), cursor: 0, anchor: None }
    }

    pub fn string(&self) -> String {
        self.text.iter().collect()
    }

    pub fn selection(&self) -> (usize, usize) {
        match self.anchor {
            None => (self.cursor, self.cursor),
            Some(anchor) => (anchor.min(self.cursor), anchor.max(self.cursor)),
        }
    }

    pub fn delete_selection(&mut self) -> bool {
        let (start, end) = self.selection();
        self.anchor = None;
        if start == end {
            return false;
        }
        self.text.drain(start..end);
        self.cursor = start;
        true
    }

    fn lines(&self) -> Vec<usize> {
        // Length of every line in characters.
        let mut lengths = vec![0];
        for c in &self.text {
            if *c == '\n' {
                lengths.push(0);
            } else {
                *lengths.last_mut().unwrap() += 1;
            }
        }
        lengths
    }

    pub fn point(&mut self, row: isize, col: isize, extend: bool) {
        if extend && self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
        if !extend {
            self.anchor = None;
        }
        let lines = self.lines();
        let row = row.clamp(0, lines.len() as isize - 1) as usize;
        self.cursor = lines[..row].iter().map(|length| length + 1).sum::<usize>() + (col.max(0) as usize).min(lines[row]);
    }

    pub fn insert(&mut self, text: &str) {
        self.delete_selection();
        let chars: Vec<char> = text.chars().collect();
        let count = chars.len();
        self.text.splice(self.cursor..self.cursor, chars);
        self.cursor += count;
    }

    pub fn position(&self) -> (usize, usize) {
        let before = &self.text[..self.cursor];
        let row = before.iter().filter(|c| **c == '\n').count();
        let col = self.cursor - before.iter().rposition(|c| *c == '\n').map_or(0, |index| index + 1);
        (row, col)
    }

    pub fn move_rows(&mut self, rows: isize) {
        let lines = self.lines();
        let (row, col) = self.position();
        let next = (row as isize + rows).clamp(0, lines.len() as isize - 1) as usize;
        self.cursor = lines[..next].iter().map(|length| length + 1).sum::<usize>() + col.min(lines[next]);
    }

    pub fn key(&mut self, name: &str) {
        if (name == "backspace" || name == "delete") && self.delete_selection() {
            return;
        }
        if ["left", "right", "up", "down", "home", "end"].contains(&name) {
            self.anchor = None;
        }
        match name {
            "left" => self.cursor = self.cursor.saturating_sub(1),
            "right" => self.cursor = (self.cursor + 1).min(self.text.len()),
            "up" => self.move_rows(-1),
            "down" => self.move_rows(1),
            "home" => self.cursor -= self.position().1,
            "end" => self.cursor = self.text[self.cursor..].iter().position(|c| *c == '\n').map_or(self.text.len(), |offset| self.cursor + offset),
            "backspace" if self.cursor > 0 => {
                self.text.remove(self.cursor - 1);
                self.cursor -= 1;
            }
            "delete" if self.cursor < self.text.len() => {
                self.text.remove(self.cursor);
            }
            "return" => self.insert("\n"),
            "tab" => self.insert("  "),
            _ => {}
        }
    }
}

#[derive(Clone)]
struct Snapshot {
    text: Vec<char>,
    cursor: usize,
    anchor: Option<usize>,
}

struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    group: Option<(String, Instant)>,
}

impl History {
    fn snapshot(buffer: &EditorBuffer) -> Snapshot {
        Snapshot { text: buffer.text.clone(), cursor: buffer.cursor, anchor: buffer.anchor }
    }

    fn record(&mut self, buffer: &EditorBuffer, before: Snapshot, kind: &str, groupable: bool) {
        if before.text == buffer.text {
            return;
        }
        let grouped = groupable && self.group.as_ref().is_some_and(|(previous, at)| previous == kind && at.elapsed() <= Duration::from_millis(600));
        if !grouped {
            self.undo.push(before);
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.group = groupable.then(|| (kind.to_string(), Instant::now()));
    }

    fn change(&mut self, buffer: &mut EditorBuffer, undo: bool) -> bool {
        let (from, to) = if undo { (&mut self.undo, &mut self.redo) } else { (&mut self.redo, &mut self.undo) };
        let Some(state) = from.pop() else { return false };
        to.push(History::snapshot(buffer));
        buffer.text = state.text;
        buffer.cursor = state.cursor;
        buffer.anchor = state.anchor;
        self.group = None;
        true
    }
}

pub fn color_line(line: &str, enabled: bool) -> String {
    let clean = safe(line);
    if !enabled {
        return clean;
    }
    re!(r#"("(?:\\.|[^"\\])*"\s*:?)|(//.*$)|\b(true|false|null|-?[0-9]+(?:\.[0-9]+)?)\b"#)
        .replace_all(&clean, |captures: &regex::Captures| {
            let token = &captures[0];
            let code = if captures.get(2).is_some() {
                90
            } else if captures.get(1).is_some() {
                if token.ends_with(':') {
                    36
                } else {
                    32
                }
            } else {
                33
            };
            format!("\x1b[{code}m{token}\x1b[0m")
        })
        .into_owned()
}

struct Editor<'a> {
    file: &'a Path,
    color: bool,
    newline: &'static str,
    buffer: EditorBuffer,
    history: History,
    saved: Vec<char>,
    disk: String,
    issue: Option<EditorIssue>,
    top: usize,
    left: usize,
    follow: bool,
    question: bool,
    guide_prompt: bool,
    status: String,
    completion: Option<(usize, usize, Vec<serde_json::Value>, usize)>,
    pending: Option<Instant>,
    viewport: (usize, usize, usize),
    dragging: bool,
}

impl Editor<'_> {
    fn validate(&mut self) {
        self.issue = validate_editor_text(&self.buffer.string());
        self.pending = None;
    }

    fn render(&mut self) {
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

    fn edited(&mut self, before: Snapshot, kind: &str, groupable: bool) {
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

    fn save(&mut self) {
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
    fn key(&mut self, key: KeyEvent) -> bool {
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

    fn paste(&mut self, pasted: &str) {
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

    fn request_exit(&mut self) -> bool {
        if self.buffer.text == self.saved {
            return true;
        }
        self.question = true;
        false
    }

    fn mouse(&mut self, kind: MouseEventKind, x: usize, y: usize) {
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
