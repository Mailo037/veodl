//! The stateful terminal reporter: animations, live progress, titles.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use crate::output::LiveRegion;
use crate::terminal::{self, columns, Std, TitleSetter};
use crate::utils::clean_text;

use super::*;

#[derive(Clone)]
pub(super) struct Animation {
    pub(super) label: String,
    pub(super) owner: Option<u64>,
    pub(super) frame: u64,
    pub(super) last: Instant,
}

#[derive(Clone)]
pub(super) enum Redraw {
    Animation,
    Progress(Progress, String),
}

pub(super) struct State {
    pub(super) color: bool,
    pub(super) region: Option<InlineRegion>,
    pub(super) active: bool,
    pub(super) last_log: Option<Instant>,
    pub(super) name: String,
    pub(super) phase: String,
    pub(super) started: bool,
    pub(super) position: String,
    pub(super) has_item: bool,
    pub(super) stream_name: String,
    pub(super) animation: Option<Animation>,
    pub(super) redraw: Option<Redraw>,
    pub(super) last_width: usize,
    pub(super) disposed: bool,
}

/// The terminal reporter: animated status rows, progress bars and titles on
/// stderr, with plain periodic log lines when stderr is redirected.
pub struct TerminalReporter {
    pub(super) tty: bool,
    pub(super) state: Mutex<State>,
    pub(super) title: TitleSetter,
    pub(super) owners: AtomicU64,
}

impl TerminalReporter {
    pub fn new() -> Arc<TerminalReporter> {
        let tty = Std::Err.is_tty();
        let reporter = Arc::new(TerminalReporter {
            tty,
            state: Mutex::new(State {
                color: true,
                region: None,
                active: false,
                last_log: None,
                name: String::new(),
                phase: "Starting…".into(),
                started: false,
                position: String::new(),
                has_item: false,
                stream_name: String::new(),
                animation: None,
                redraw: None,
                last_width: columns(),
                disposed: false,
            }),
            title: TitleSetter::new(),
            owners: AtomicU64::new(1),
        });
        if tty {
            let weak = Arc::downgrade(&reporter);
            std::thread::spawn(move || ticker(weak));
        }
        reporter
    }

    pub(super) fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    pub(super) fn ctx(&self, state: &State) -> Ctx {
        Ctx { tty: self.tty, color: state.color }
    }

    pub(super) fn append(&self, state: &mut State, piece: Piece) {
        let ctx = self.ctx(state);
        match &mut state.region {
            Some(region) => {
                region.logs.push(piece);
                region.render(&ctx);
            }
            None => Std::Err.write(&ctx.render(&piece)),
        }
    }

    pub(super) fn erase_live(state: &State, out: &mut String) {
        if state.active {
            out.push_str("\r\x1b[2K\x1b[J");
        }
    }

    pub(super) fn render_live(&self, state: &mut State, text: &str) {
        let mut out = String::new();
        Self::erase_live(state, &mut out);
        let rows = text.split('\n').count();
        // Park the cursor at the block's first row; the terminal relocates it on reflow.
        out.push_str(&format!("\r\x1b[2K{}{}\r", text.replace('\n', "\r\n"), if rows > 1 { format!("\x1b[{}A", rows - 1) } else { String::new() }));
        Std::Err.write(&out);
        state.active = true;
    }

    pub(super) fn set_live(&self, state: &mut State, piece: Option<Piece>) {
        let ctx = self.ctx(state);
        if let Some(region) = &mut state.region {
            region.live = piece;
            region.render(&ctx);
        }
    }

    pub(super) fn redraw(&self, state: &mut State) {
        match state.redraw.clone() {
            Some(Redraw::Animation) => self.tick(state),
            Some(Redraw::Progress(data, prefix)) => {
                let text = self.ctx(state).render(&Piece::Progress { data, prefix });
                self.render_live(state, &text);
            }
            None => {}
        }
    }

    pub(super) fn watch_resize(&self, state: &mut State, redraw: Option<Redraw>) {
        if state.region.is_some() {
            return;
        }
        if self.tty && columns() != state.last_width {
            state.last_width = columns();
            if state.active {
                self.redraw(state);
            }
        }
        state.redraw = redraw;
    }

    pub(super) fn clear(&self, state: &mut State) {
        state.animation = None;
        if state.region.is_some() {
            self.set_live(state, None);
        }
        state.redraw = None;
        if state.active && self.tty {
            let mut out = String::new();
            Self::erase_live(state, &mut out);
            out.push_str("\r\x1b[2K");
            Std::Err.write(&out);
        }
        state.active = false;
    }

    pub(super) fn tick(&self, state: &mut State) {
        let Some(animation) = state.animation.as_mut() else { return };
        let piece = Piece::Animation { label: animation.label.clone(), frame: animation.frame };
        animation.frame += 1;
        animation.last = Instant::now();
        if state.region.is_some() {
            self.set_live(state, Some(piece));
        } else {
            let text = self.ctx(state).render(&piece);
            self.render_live(state, &text);
        }
    }

    pub(super) fn animate(&self, state: &mut State, label: &str, owner: Option<u64>) {
        self.clear(state);
        if !self.tty {
            Std::Err.write(&format!("{}\n", self.ctx(state).paint(&format!("{label}…"), "muted")));
            return;
        }
        state.animation = Some(Animation { label: label.to_string(), owner, frame: 0, last: Instant::now() });
        self.watch_resize(state, Some(Redraw::Animation));
        self.tick(state);
    }

    pub(super) fn processing_result(&self, state: &mut State, label: &str, status: &str) {
        self.clear(state);
        self.append(state, Piece::Result { label: label.to_string(), failed: matches!(status, "failed" | "error") });
        self.watch_resize(state, None);
    }

    pub(super) fn finish_status_for(&self, state: &mut State, message: &str, owner: Option<u64>, prefix: &str, outcome: Option<&str>) {
        let label = format!("{prefix}{}", clean_text(message));
        let base = label.strip_suffix('…').unwrap_or(&label[..label.len().saturating_sub(label.chars().last().map_or(0, char::len_utf8))]).to_string();
        let Some(animation) = &state.animation else { return };
        if animation.owner != owner || animation.label != base {
            return;
        }
        if let Some(outcome) = outcome.filter(|outcome| !outcome.is_empty()) {
            self.processing_result(state, &base, outcome);
            return;
        }
        self.clear(state);
        self.append(state, Piece::Muted(label));
        self.watch_resize(state, None);
    }

    pub(super) fn draw(&self, state: &mut State, data: &Progress, prefix: String) {
        let piece = Piece::Progress { data: data.clone(), prefix: prefix.clone() };
        if state.region.is_some() {
            self.set_live(state, Some(piece));
            return;
        }
        let text = self.ctx(state).render(&piece);
        self.watch_resize(state, Some(Redraw::Progress(data.clone(), prefix)));
        self.render_live(state, &text);
    }

    pub(super) fn update_title(&self, state: &State) {
        if state.started {
            let name = if state.name.is_empty() { String::new() } else { format!(" | {}", state.name) };
            self.title.set(self.tty, &format!("veo | {}{name}", state.phase));
        }
    }

    pub(super) fn fail_step_for(&self, state: &mut State, owner: Option<u64>) {
        if let Some(animation) = state.animation.clone().filter(|animation| animation.owner == owner) {
            self.processing_result(state, &animation.label, "failed");
        }
    }

    /// Switch to the inline region for downloads on an interactive terminal.
    pub fn enable_inline(&self) {
        let mut state = self.lock();
        if state.region.is_none() && self.tty && !terminal::dumb() {
            self.clear(&mut state);
            state.region = Some(InlineRegion::default());
        }
    }

    pub fn dispose(&self) {
        let mut state = self.lock();
        self.clear(&mut state);
        let ctx = self.ctx(&state);
        if let Some(mut region) = state.region.take() {
            region.commit(&ctx);
        }
        state.disposed = true;
    }

    pub fn configure(&self, color: bool) {
        self.lock().color = color;
    }

    pub fn start(&self, title: &str) {
        let mut state = self.lock();
        state.started = true;
        state.name = clean_text(title);
        state.phase = "Starting…".into();
        self.update_title(&state);
    }

    pub fn profile(&self, value: Option<&str>) {
        let mut state = self.lock();
        let selected = value.map(clean_text).unwrap_or_else(|| "global (no profile)".into());
        state.phase = format!("Profile: {selected}");
        self.update_title(&state);
        self.clear(&mut state);
        self.append(&mut state, Piece::Profile(value.map(str::to_string)));
    }

    pub fn complete(&self) {
        let mut state = self.lock();
        self.clear(&mut state);
        state.phase = "Done".into();
        self.update_title(&state);
    }

    pub fn fail(&self, cancelled: bool) {
        let mut state = self.lock();
        self.clear(&mut state);
        state.phase = if cancelled { "Cancelled" } else { "Failed" }.into();
        self.update_title(&state);
    }

    pub(super) fn scoped_child(self: &Arc<Self>, index: usize, total: usize, title: &str, parent: &str) -> Arc<dyn Reporter> {
        Arc::new(Scoped {
            root: self.clone(),
            owner: self.owners.fetch_add(1, Ordering::SeqCst),
            prefix: format!("{parent}[{index}/{total}] "),
            child_name: Mutex::new(clean_text(title)),
            last_progress: Mutex::new(None),
        })
    }
}

pub(super) fn ticker(reporter: Weak<TerminalReporter>) {
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let Some(reporter) = reporter.upgrade() else { return };
        let mut state = reporter.lock();
        if state.disposed {
            return;
        }
        let ctx = reporter.ctx(&state);
        if let Some(region) = &mut state.region {
            region.resize(&ctx);
        } else if columns() != state.last_width {
            state.last_width = columns();
            if state.active {
                reporter.redraw(&mut state);
            }
        }
        if state.animation.as_ref().is_some_and(|animation| animation.last.elapsed() >= Duration::from_millis(350)) {
            reporter.tick(&mut state);
        }
    }
}

impl LiveRegion for TerminalReporter {
    fn before_output(&self) {
        let mut state = self.lock();
        self.clear(&mut state);
        let ctx = self.ctx(&state);
        if let Some(region) = &mut state.region {
            region.commit(&ctx);
        }
    }
}
