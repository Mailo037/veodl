//! Text buffer, undo history, validation and syntax colors of the editor.

use std::time::{Duration, Instant};

use crate::config::parse_config_text;
use crate::config_diagnostics::semantic_issue;
use crate::error::Error;

pub(super) fn safe(text: &str) -> String {
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

pub(super) fn char_offset(text: &str, byte: usize) -> usize {
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

    pub(super) fn lines(&self) -> Vec<usize> {
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
pub(super) struct Snapshot {
    pub(super) text: Vec<char>,
    pub(super) cursor: usize,
    pub(super) anchor: Option<usize>,
}

pub(super) struct History {
    pub(super) undo: Vec<Snapshot>,
    pub(super) redo: Vec<Snapshot>,
    pub(super) group: Option<(String, Instant)>,
}

impl History {
    pub(super) fn snapshot(buffer: &EditorBuffer) -> Snapshot {
        Snapshot { text: buffer.text.clone(), cursor: buffer.cursor, anchor: buffer.anchor }
    }

    pub(super) fn record(&mut self, buffer: &EditorBuffer, before: Snapshot, kind: &str, groupable: bool) {
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

    pub(super) fn change(&mut self, buffer: &mut EditorBuffer, undo: bool) -> bool {
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
