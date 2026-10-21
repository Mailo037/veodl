//! Rows that re-render at the current width, and the inline terminal region.

use crate::terminal::{self, cells, columns, fit, style, terminal_text, terminal_title, word_wrap, Std};
use crate::utils::clean_text;

use super::*;

/// What a rendered row shows. Rows re-render at the current terminal width.
#[derive(Clone, Debug)]
pub(super) enum Piece {
    Title { text: String, muted: bool },
    Status(String),
    Muted(String),
    Line { text: String, quiet: bool },
    Result { label: String, failed: bool },
    Profile(Option<String>),
    Progress { data: Progress, prefix: String },
    Animation { label: String, frame: u64 },
}

pub(super) struct Ctx {
    pub(super) tty: bool,
    pub(super) color: bool,
}

impl Ctx {
    pub(super) fn paint(&self, text: &str, role: &str) -> String {
        if text.is_empty() {
            String::new()
        } else {
            style(self.tty, text, role, self.color)
        }
    }

    pub(super) fn width(&self) -> Option<usize> {
        terminal::line_width(self.tty)
    }

    pub(super) fn display(&self, text: &str) -> String {
        terminal_text(self.tty, text)
    }

    // Status notes wrap at word boundaries, at most three rows.
    pub(super) fn block(&self, value: &str) -> Vec<String> {
        let width = self.width();
        let rows: Vec<String> = word_wrap(&clean_text(value), width).split('\n').map(str::to_string).collect();
        if rows.len() <= 3 {
            return rows;
        }
        vec![rows[0].clone(), rows[1].clone(), fit(&rows[2..].join(" "), width.unwrap_or(usize::MAX))]
    }

    pub(super) fn render(&self, piece: &Piece) -> String {
        match piece {
            Piece::Title { text, muted } => self.paint(&terminal_title(self.tty, text), if *muted { "muted" } else { "title" }) + "\n",
            Piece::Status(text) => {
                let stripped = re!(r"^(?:\[[0-9]+/[0-9]+\] )+[^:]*: ").replace(&clean_text(text), "").into_owned();
                let role = if re!(r"^(?:Playback note:|Playback compatibility could not|Warning:)").is_match(&stripped) { "warn" } else { "muted" };
                self.block(text).iter().map(|row| self.paint(row, role)).collect::<Vec<_>>().join("\n") + "\n"
            }
            Piece::Muted(text) => self.paint(&self.display(text), "muted") + "\n",
            Piece::Line { text, quiet } => {
                let shown = self.display(text);
                (if *quiet { self.paint(&shown, "muted") } else { shown }) + "\n"
            }
            Piece::Result { label, failed } => {
                let suffix = if *failed { "failed" } else { "done" };
                let width = self.width().unwrap_or(usize::MAX).saturating_sub(suffix.len() + 2);
                self.paint(&format!("{}: ", fit(label, width)), "muted") + &self.paint(suffix, if *failed { "error" } else { "success" }) + "\n"
            }
            Piece::Profile(value) => match value.as_deref().filter(|value| *value != "default") {
                Some(value) => {
                    let width = if self.tty { columns().saturating_sub(10) } else { usize::MAX };
                    self.paint("Profile: ", "muted") + &self.paint(&fit(&clean_text(value), width), "profile") + "\n"
                }
                None => {
                    let selected = value.as_deref().map(clean_text).unwrap_or_else(|| "global (no profile)".into());
                    self.paint(&self.display(&format!("Profile: {selected}")), "muted") + "\n"
                }
            },
            Piece::Progress { data, prefix } => {
                let paint = |text: &str, role: &str| self.paint(text, role);
                format_progress(
                    data,
                    &ProgressStyle { columns: self.width(), prefix, unicode: terminal::supports_unicode(self.tty), paint: &paint, frame: frame_now() },
                )
            }
            Piece::Animation { label, frame } => self.paint(&self.display(&format!("{label}{}", ".".repeat((*frame % 3 + 1) as usize))), "muted"),
        }
    }
}

pub(super) fn strip_vt(text: &str) -> String {
    re!(r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\))").replace_all(text, "").into_owned()
}

/// A bounded region in the normal terminal buffer, with the cursor at its end.
#[derive(Default)]
pub(super) struct InlineRegion {
    pub(super) source: Option<Piece>,
    pub(super) logs: Vec<Piece>,
    pub(super) live: Option<Piece>,
    pub(super) visible: bool,
    pub(super) rendered: Vec<String>,
    pub(super) last_size: (usize, usize),
}

impl InlineRegion {
    pub(super) fn text(ctx: &Ctx, piece: &Piece) -> String {
        let text = ctx.render(piece);
        text.strip_suffix('\n').unwrap_or(&text).replace("\r\n", "\n")
    }

    pub(super) fn erase(&mut self, out: &mut String) {
        if !self.visible {
            return;
        }
        let width = columns().max(1);
        let last_reflows = cfg!(windows);
        let count = self.rendered.len();
        let above: usize = self
            .rendered
            .iter()
            .enumerate()
            .map(|(index, line)| if index == count - 1 && !last_reflows { 1 } else { cells(&strip_vt(line)).div_ceil(width).max(1) })
            .sum::<usize>()
            .saturating_sub(1);
        if above > 0 {
            out.push_str(&format!("\x1b[{above}A"));
        }
        out.push_str("\r\x1b[2K\x1b[J");
    }

    pub(super) fn render(&mut self, ctx: &Ctx) {
        let mut out = String::new();
        let width = columns();
        let height = terminal::rows().saturating_sub(1).max(1);
        self.last_size = (width, height);
        self.erase(&mut out);
        let source_rows: Vec<String> = self.source.as_ref().map(|piece| Self::text(ctx, piece).split('\n').map(str::to_string).collect()).unwrap_or_default();
        let progress: Vec<String> = self.live.as_ref().map(|piece| Self::text(ctx, piece).split('\n').map(str::to_string).collect()).unwrap_or_default();
        let log_lines = |logs: &[Piece]| -> Vec<String> {
            logs.iter().flat_map(|piece| Self::text(ctx, piece).split('\n').map(str::to_string).collect::<Vec<_>>()).collect()
        };
        // Commit older status messages above the region before it outgrows the viewport.
        while !self.logs.is_empty() && source_rows.len() + log_lines(&self.logs).len() + progress.len() > height {
            let first = self.logs.remove(0);
            out.push_str(&(Self::text(ctx, &first).replace('\n', "\r\n") + "\r\n"));
        }
        let lines: Vec<String> = source_rows.into_iter().chain(log_lines(&self.logs)).chain(progress).collect();
        if lines.is_empty() {
            self.visible = false;
        } else {
            out.push('\r');
            out.push_str(&lines.join("\r\n"));
            self.rendered = lines;
            self.visible = true;
        }
        Std::Err.write(&out);
    }

    pub(super) fn resize(&mut self, ctx: &Ctx) {
        if self.last_size != (columns(), terminal::rows().saturating_sub(1).max(1)) {
            self.render(ctx);
        }
    }

    pub(super) fn commit(&mut self, ctx: &Ctx) {
        let mut out = String::new();
        self.erase(&mut out);
        let lines: Vec<String> = self.source.iter().chain(self.logs.iter()).map(|piece| Self::text(ctx, piece)).collect();
        if !lines.is_empty() {
            out.push_str(&(lines.join("\n").replace('\n', "\r\n") + "\r\n"));
        }
        Std::Err.write(&out);
        self.source = None;
        self.logs.clear();
        self.live = None;
        self.visible = false;
    }
}
