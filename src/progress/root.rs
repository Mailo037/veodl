//! Reporter implementations for the whole run and for parallel items.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::terminal::Std;
use crate::utils::clean_text;

use super::*;

/// Lets `Arc<TerminalReporter>` serve as `Arc<dyn Reporter>` while keeping
/// access to scoped child reporters.
pub struct Root(pub Arc<TerminalReporter>);

impl Reporter for Root {
    fn status(&self, message: &str) {
        let root = &self.0;
        let mut state = root.lock();
        state.phase = clean_text(message);
        root.update_title(&state);
        if let Some(label) = state.phase.strip_suffix('…').map(str::to_string) {
            root.animate(&mut state, &label, None);
        } else {
            root.clear(&mut state);
            let phase = state.phase.clone();
            root.append(&mut state, Piece::Status(phase));
            root.watch_resize(&mut state, None);
        }
    }

    fn finish_status(&self, message: &str, outcome: &str) {
        let mut state = self.0.lock();
        self.0.finish_status_for(&mut state, message, None, "", Some(outcome));
    }

    fn progress(&self, data: &Progress) {
        let root = &self.0;
        let mut state = root.lock();
        if let Some(stream) = data.stream.as_deref().filter(|stream| *stream != state.stream_name) {
            root.clear(&mut state);
            state.stream_name = stream.to_string();
            state.last_log = None;
            let label = format!("{}{} download", state.position, state.stream_name);
            root.append(&mut state, Piece::Muted(label));
        }
        let percent = match (data.total_bytes.filter(|total| total.is_finite() && *total > 0.0), data.downloaded_bytes.filter(|done| done.is_finite())) {
            (Some(total), Some(done)) => Some((done / total * 100.0).round().clamp(0.0, 100.0)),
            _ => None,
        };
        state.phase = if data.finished() { "Processing…".into() } else { percent.map_or("Downloading…".into(), |percent| format!("{percent}%")) };
        root.update_title(&state);
        let prefix = format!("{}{}", state.position, state.stream_name);
        if root.tty {
            root.draw(&mut state, data, prefix);
        } else if state.last_log.is_none_or(|last| last.elapsed() >= Duration::from_secs(5)) || data.finished() {
            let stream = if state.stream_name.is_empty() { String::new() } else { format!("{} ", state.stream_name) };
            Std::Err.write(&format!("{}{stream}{}\n", state.position, format_progress(data, &ProgressStyle::default())));
            state.last_log = Some(Instant::now());
        }
    }

    fn processing(&self, data: &Processing) {
        let root = &self.0;
        let mut state = root.lock();
        let processor = clean_text(data.postprocessor.as_deref().unwrap_or("Processing"));
        let label = processing_label(&processor);
        state.phase = format!("{label}{}", if data.status.as_deref() == Some("finished") { ": done" } else { "…" });
        root.update_title(&state);
        let text = format!("{}{label}", state.position);
        if ended(data.status.as_deref()) {
            root.processing_result(&mut state, &text, data.status.as_deref().unwrap_or("finished"));
        } else {
            root.animate(&mut state, &text, None);
        }
    }

    fn name(&self, title: &str) {
        let root = &self.0;
        let mut state = root.lock();
        let next = clean_text(title);
        if state.has_item && next != state.name {
            root.clear(&mut state);
            let piece = Piece::Title { text: format!("{}{next}", state.position), muted: false };
            root.append(&mut state, piece);
            root.watch_resize(&mut state, None);
        }
        state.name = next;
        root.update_title(&state);
    }

    fn item(&self, index: usize, total: usize, title: &str) {
        let root = &self.0;
        let mut state = root.lock();
        root.clear(&mut state);
        state.position = if total > 1 { format!("[{index}/{total}] ") } else { String::new() };
        state.has_item = true;
        state.name = clean_text(title);
        state.stream_name.clear();
        state.last_log = None;
        // The URL only identifies the request; the media title that follows is the heading.
        let piece = Piece::Title { text: format!("{}{}", state.position, state.name), muted: re!(r"(?i)^https?://").is_match(&state.name) };
        let ctx = root.ctx(&state);
        match &mut state.region {
            Some(region) => {
                region.commit(&ctx);
                region.source = Some(piece);
                region.render(&ctx);
            }
            None => root.append(&mut state, piece),
        }
        root.watch_resize(&mut state, None);
        state.phase = "Starting…".into();
        root.update_title(&state);
    }

    fn scoped(&self, index: usize, total: usize, title: &str) -> Option<Arc<dyn Reporter>> {
        Some(self.0.scoped_child(index, total, title, ""))
    }

    fn fail_step(&self) {
        let mut state = self.0.lock();
        self.0.fail_step_for(&mut state, None);
    }

    fn finish(&self) {
        let root = &self.0;
        let mut state = root.lock();
        root.clear(&mut state);
        let ctx = root.ctx(&state);
        if let Some(region) = &mut state.region {
            region.commit(&ctx);
        }
    }

    fn complete(&self) {
        self.0.complete();
    }

    fn fail(&self, cancelled: bool) {
        self.0.fail(cancelled);
    }
}

/// Reporter for one of several parallel items: every row carries `[i/n] title:`.
pub(super) struct Scoped {
    pub(super) root: Arc<TerminalReporter>,
    pub(super) owner: u64,
    pub(super) prefix: String,
    pub(super) child_name: Mutex<String>,
    pub(super) last_progress: Mutex<Option<Instant>>,
}

impl Scoped {
    pub(super) fn child_name(&self) -> String {
        self.child_name.lock().unwrap_or_else(|poison| poison.into_inner()).clone()
    }

    pub(super) fn log(&self, message: &str, quiet: bool) {
        let root = &self.root;
        let mut state = root.lock();
        let name = self.child_name();
        state.phase = clean_text(message);
        state.name = name.clone();
        let line = format!("{}{name}: {}", self.prefix, state.phase);
        if let Some(label) = line.strip_suffix('…') {
            root.animate(&mut state, label, Some(self.owner));
        } else {
            root.clear(&mut state);
            root.append(&mut state, Piece::Line { text: line, quiet });
        }
        root.update_title(&state);
    }
}

impl Reporter for Scoped {
    fn status(&self, message: &str) {
        self.log(message, true);
    }

    fn finish_status(&self, message: &str, outcome: &str) {
        let mut state = self.root.lock();
        let prefix = format!("{}{}: ", self.prefix, self.child_name());
        self.root.finish_status_for(&mut state, message, Some(self.owner), &prefix, Some(outcome));
    }

    fn progress(&self, data: &Progress) {
        let stream = data.stream.clone().unwrap_or_else(|| "Media".into());
        if self.root.tty {
            let mut state = self.root.lock();
            let prefix = format!("{}{}: {stream}", self.prefix, self.child_name());
            self.root.draw(&mut state, data, prefix);
            return;
        }
        let mut last = self.last_progress.lock().unwrap_or_else(|poison| poison.into_inner());
        if last.is_none_or(|last| last.elapsed() >= Duration::from_secs(5)) || data.finished() {
            *last = Some(Instant::now());
            drop(last);
            self.log(&format!("{stream} {}", format_progress(data, &ProgressStyle::default())), false);
        }
    }

    fn processing(&self, data: &Processing) {
        let root = &self.root;
        let mut state = root.lock();
        let name = self.child_name();
        let label_text = processing_label(data.postprocessor.as_deref().unwrap_or(""));
        let label = format!("{}{name}: {label_text}", self.prefix);
        if ended(data.status.as_deref()) {
            root.processing_result(&mut state, &label, data.status.as_deref().unwrap_or("finished"));
        } else {
            root.animate(&mut state, &label, Some(self.owner));
        }
        state.phase = format!("{label_text}{}", if data.status.as_deref() == Some("finished") { ": done" } else { "…" });
        state.name = name;
        root.update_title(&state);
    }

    fn name(&self, title: &str) {
        *self.child_name.lock().unwrap_or_else(|poison| poison.into_inner()) = clean_text(title);
    }

    fn scoped(&self, index: usize, total: usize, title: &str) -> Option<Arc<dyn Reporter>> {
        Some(self.root.scoped_child(index, total, title, &self.prefix))
    }

    fn fail_step(&self) {
        let mut state = self.root.lock();
        self.root.fail_step_for(&mut state, Some(self.owner));
    }
}
