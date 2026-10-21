use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::output::LiveRegion;
use crate::terminal::{self, cells, columns, fit, style, terminal_text, terminal_title, word_wrap, wrap, Std, TitleSetter};
use crate::utils::clean_text;

/// One yt-dlp progress report (`--progress-template` JSON).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Progress {
    #[serde(deserialize_with = "crate::metadata::text")]
    pub status: Option<String>,
    #[serde(deserialize_with = "crate::metadata::number")]
    pub downloaded_bytes: Option<f64>,
    #[serde(deserialize_with = "crate::metadata::number")]
    pub total_bytes: Option<f64>,
    #[serde(deserialize_with = "crate::metadata::number")]
    pub total_bytes_estimate: Option<f64>,
    #[serde(deserialize_with = "crate::metadata::number")]
    pub speed: Option<f64>,
    #[serde(deserialize_with = "crate::metadata::number")]
    pub eta: Option<f64>,
    #[serde(skip)]
    pub stream: Option<String>,
}

impl Progress {
    pub fn finished(&self) -> bool {
        self.status.as_deref() == Some("finished")
    }
}

/// One yt-dlp post-processing report.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Processing {
    #[serde(deserialize_with = "crate::metadata::text")]
    pub status: Option<String>,
    #[serde(deserialize_with = "crate::metadata::text")]
    pub postprocessor: Option<String>,
}

fn processing_label(processor: &str) -> &'static str {
    match processor {
        "Merger" => "Merging audio/video",
        "VideoRemuxer" => "Changing video container",
        "VideoConvertor" => "Converting video",
        "ExtractAudio" => "Converting audio",
        "EmbedSubtitle" => "Embedding subtitles",
        "Metadata" => "Writing metadata",
        "EmbedThumbnail" => "Embedding thumbnail",
        "MoveFiles" => "Preparing saved file",
        _ => "Processing media",
    }
}

fn ended(status: Option<&str>) -> bool {
    matches!(status, Some("finished" | "failed" | "error"))
}

/// `Number#toFixed` for non-negative values (ties round up, not to even).
pub fn to_fixed(value: f64, digits: usize) -> String {
    let scale = 10f64.powi(digits as i32);
    format!("{:.*}", digits, (value * scale).round() / scale)
}

pub fn bytes(value: Option<f64>) -> String {
    let Some(mut value) = value.filter(|value| value.is_finite() && *value >= 0.0) else { return "?".into() };
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut index = 0;
    while value >= 1024.0 && index < units.len() - 1 {
        value /= 1024.0;
        index += 1;
    }
    format!("{} {}", to_fixed(value, if index > 0 { 1 } else { 0 }), units[index])
}

fn pad_start(text: &str, width: usize) -> String {
    format!("{}{text}", " ".repeat(width.saturating_sub(text.chars().count())))
}

fn pad_end(text: &str, width: usize) -> String {
    format!("{text}{}", " ".repeat(width.saturating_sub(text.chars().count())))
}

const BAR_WIDTH: usize = 20;

fn frame_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|time| time.as_millis() as u64 / 120).unwrap_or(0)
}

// Fixed-width fields keep the bar and statistics from jumping between updates.
fn progress_bar(percent: Option<f64>, size: usize, unicode: bool, paint: &dyn Fn(&str, &str) -> String, frame: u64) -> String {
    let (full, empty) = if unicode { ("█", "░") } else { ("=", "-") };
    let body = match percent {
        None => {
            // Unknown total: a short block travels back and forth instead of an empty bar.
            let block = size.saturating_sub(1).clamp(1, 3);
            let span = size.saturating_sub(block).max(1) as u64;
            let step = frame % (span * 2);
            let start = (if step < span { step } else { span * 2 - step }) as usize;
            paint(&empty.repeat(start), "muted") + &paint(&full.repeat(block), "accent") + &paint(&empty.repeat(size.saturating_sub(block + start)), "muted")
        }
        Some(percent) => {
            let filled = ((percent / 100.0 * size as f64).round() as usize).min(size);
            paint(&full.repeat(filled), "accent") + &paint(&empty.repeat(size - filled), "muted")
        }
    };
    if unicode {
        body
    } else {
        format!("[{body}]")
    }
}

pub struct ProgressStyle<'a> {
    pub columns: Option<usize>,
    pub prefix: &'a str,
    pub unicode: bool,
    pub paint: &'a dyn Fn(&str, &str) -> String,
    pub frame: u64,
}

fn no_paint(text: &str, _: &str) -> String {
    text.to_string()
}

impl Default for ProgressStyle<'_> {
    fn default() -> Self {
        ProgressStyle { columns: None, prefix: "", unicode: false, paint: &no_paint, frame: frame_now() }
    }
}

/// Render one progress update; plain ASCII by default, as used in logs.
pub fn format_progress(data: &Progress, options: &ProgressStyle) -> String {
    let finite = |value: Option<f64>| value.filter(|value| value.is_finite());
    let done = finite(data.downloaded_bytes).unwrap_or(0.0);
    let exact = data.total_bytes.filter(|total| *total != 0.0 && !total.is_nan());
    let total = exact.or(data.total_bytes_estimate.filter(|total| *total != 0.0 && !total.is_nan()));
    let estimated = exact.is_none() && total.is_some();
    let finished = data.finished();
    let percent = total.filter(|total| *total > 0.0).map(|total| (done / total * 100.0).clamp(0.0, if estimated && !finished { 99.0 } else { 100.0 }));
    let eta = match finite(data.eta) {
        Some(eta) => format!("{}:{:02}", (eta / 60.0).floor() as i64, (eta % 60.0).floor() as i64),
        None => "?".into(),
    };
    let pct = pad_start(&format!("{}{}%", if estimated && !finished { "~" } else { "" }, percent.map_or("?".into(), |percent| to_fixed(percent, 0))), 4);
    let size = match total.filter(|total| *total > 0.0) {
        Some(total) => format!("{} / {}{}", bytes(Some(done)), if estimated { "~" } else { "" }, bytes(Some(total))),
        None => format!("{} received", bytes(Some(done))),
    };
    let label = if options.prefix.is_empty() { String::new() } else { format!("{} ", clean_text(options.prefix)) };
    if finished {
        return wrap(&format!("{label}{} received; processing…", bytes(Some(done))), options.columns);
    }
    let stats = [pad_start(&format!("{}/s", bytes(data.speed)), 11), pad_start(&size, 19), pad_end(&format!("ETA {eta}"), 9)];
    let decoration = if options.unicode { 1 } else { 3 };
    let bar_size = match options.columns {
        Some(columns) => columns.saturating_sub(cells(&pct) + decoration).clamp(1, BAR_WIDTH),
        None => BAR_WIDTH,
    };
    let plain_bar = format!("{} {pct}", progress_bar(percent, bar_size, options.unicode, &no_paint, options.frame));
    let bar = format!("{} {pct}", progress_bar(percent, bar_size, options.unicode, options.paint, options.frame));
    let columns = options.columns.unwrap_or(usize::MAX);
    let full = format!("{label}{plain_bar} {}", stats.join("  "));
    if cells(full.trim_end()) <= columns {
        return format!("{label}{bar} {}", stats.join("  ")).trim_end().to_string();
    }
    // Too narrow for aligned fields: keep one line without padding if possible.
    let compact = stats.iter().map(|value| value.trim()).collect::<Vec<_>>().join("  ");
    if cells(&format!("{label}{plain_bar}  {compact}")) <= columns {
        return format!("{label}{bar}  {compact}");
    }
    let mut lines = Vec::new();
    if !label.is_empty() {
        lines.push(fit(label.trim_end(), columns));
    }
    lines.push(if cells(&plain_bar) <= columns { bar } else { wrap(&plain_bar, options.columns) });
    let mut stat_line = String::new();
    for stat in stats.iter().map(|value| value.trim()) {
        if !stat_line.is_empty() && cells(&format!("{stat_line}  {stat}")) > columns {
            lines.push(wrap(&stat_line, options.columns));
            stat_line.clear();
        }
        if !stat_line.is_empty() {
            stat_line.push_str("  ");
        }
        stat_line.push_str(stat);
    }
    if !stat_line.is_empty() {
        lines.push(wrap(&stat_line, options.columns));
    }
    lines.join("\n")
}

/// Progress and status sink used by downloads and tool setup.
pub trait Reporter: Send + Sync {
    fn status(&self, _message: &str) {}
    fn finish_status(&self, _message: &str, _outcome: &str) {}
    fn progress(&self, _data: &Progress) {}
    fn processing(&self, _data: &Processing) {}
    fn name(&self, _title: &str) {}
    fn item(&self, _index: usize, _total: usize, _title: &str) {}
    fn scoped(&self, _index: usize, _total: usize, _title: &str) -> Option<Arc<dyn Reporter>> {
        None
    }
    fn fail_step(&self) {}
    fn finish(&self) {}
    /// The whole run succeeded (terminal title only).
    fn complete(&self) {}
    /// The whole run failed or was cancelled.
    fn fail(&self, _cancelled: bool) {}
}

/// Discards everything; used where nobody watches.
pub struct NullReporter;

impl Reporter for NullReporter {}

/// What a rendered row shows. Rows re-render at the current terminal width.
#[derive(Clone, Debug)]
enum Piece {
    Title { text: String, muted: bool },
    Status(String),
    Muted(String),
    Line { text: String, quiet: bool },
    Result { label: String, failed: bool },
    Profile(Option<String>),
    Progress { data: Progress, prefix: String },
    Animation { label: String, frame: u64 },
}

struct Ctx {
    tty: bool,
    color: bool,
}

impl Ctx {
    fn paint(&self, text: &str, role: &str) -> String {
        if text.is_empty() {
            String::new()
        } else {
            style(self.tty, text, role, self.color)
        }
    }

    fn width(&self) -> Option<usize> {
        terminal::line_width(self.tty)
    }

    fn display(&self, text: &str) -> String {
        terminal_text(self.tty, text)
    }

    // Status notes wrap at word boundaries, at most three rows.
    fn block(&self, value: &str) -> Vec<String> {
        let width = self.width();
        let rows: Vec<String> = word_wrap(&clean_text(value), width).split('\n').map(str::to_string).collect();
        if rows.len() <= 3 {
            return rows;
        }
        vec![rows[0].clone(), rows[1].clone(), fit(&rows[2..].join(" "), width.unwrap_or(usize::MAX))]
    }

    fn render(&self, piece: &Piece) -> String {
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

fn strip_vt(text: &str) -> String {
    re!(r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\))").replace_all(text, "").into_owned()
}

/// A bounded region in the normal terminal buffer, with the cursor at its end.
#[derive(Default)]
struct InlineRegion {
    source: Option<Piece>,
    logs: Vec<Piece>,
    live: Option<Piece>,
    visible: bool,
    rendered: Vec<String>,
    last_size: (usize, usize),
}

impl InlineRegion {
    fn text(ctx: &Ctx, piece: &Piece) -> String {
        let text = ctx.render(piece);
        text.strip_suffix('\n').unwrap_or(&text).replace("\r\n", "\n")
    }

    fn erase(&mut self, out: &mut String) {
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

    fn render(&mut self, ctx: &Ctx) {
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

    fn resize(&mut self, ctx: &Ctx) {
        if self.last_size != (columns(), terminal::rows().saturating_sub(1).max(1)) {
            self.render(ctx);
        }
    }

    fn commit(&mut self, ctx: &Ctx) {
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

#[derive(Clone)]
struct Animation {
    label: String,
    owner: Option<u64>,
    frame: u64,
    last: Instant,
}

#[derive(Clone)]
enum Redraw {
    Animation,
    Progress(Progress, String),
}

struct State {
    color: bool,
    region: Option<InlineRegion>,
    active: bool,
    last_log: Option<Instant>,
    name: String,
    phase: String,
    started: bool,
    position: String,
    has_item: bool,
    stream_name: String,
    animation: Option<Animation>,
    redraw: Option<Redraw>,
    last_width: usize,
    disposed: bool,
}

/// The terminal reporter: animated status rows, progress bars and titles on
/// stderr, with plain periodic log lines when stderr is redirected.
pub struct TerminalReporter {
    tty: bool,
    state: Mutex<State>,
    title: TitleSetter,
    owners: AtomicU64,
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

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    fn ctx(&self, state: &State) -> Ctx {
        Ctx { tty: self.tty, color: state.color }
    }

    fn append(&self, state: &mut State, piece: Piece) {
        let ctx = self.ctx(state);
        match &mut state.region {
            Some(region) => {
                region.logs.push(piece);
                region.render(&ctx);
            }
            None => Std::Err.write(&ctx.render(&piece)),
        }
    }

    fn erase_live(state: &State, out: &mut String) {
        if state.active {
            out.push_str("\r\x1b[2K\x1b[J");
        }
    }

    fn render_live(&self, state: &mut State, text: &str) {
        let mut out = String::new();
        Self::erase_live(state, &mut out);
        let rows = text.split('\n').count();
        // Park the cursor at the block's first row; the terminal relocates it on reflow.
        out.push_str(&format!("\r\x1b[2K{}{}\r", text.replace('\n', "\r\n"), if rows > 1 { format!("\x1b[{}A", rows - 1) } else { String::new() }));
        Std::Err.write(&out);
        state.active = true;
    }

    fn set_live(&self, state: &mut State, piece: Option<Piece>) {
        let ctx = self.ctx(state);
        if let Some(region) = &mut state.region {
            region.live = piece;
            region.render(&ctx);
        }
    }

    fn redraw(&self, state: &mut State) {
        match state.redraw.clone() {
            Some(Redraw::Animation) => self.tick(state),
            Some(Redraw::Progress(data, prefix)) => {
                let text = self.ctx(state).render(&Piece::Progress { data, prefix });
                self.render_live(state, &text);
            }
            None => {}
        }
    }

    fn watch_resize(&self, state: &mut State, redraw: Option<Redraw>) {
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

    fn clear(&self, state: &mut State) {
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

    fn tick(&self, state: &mut State) {
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

    fn animate(&self, state: &mut State, label: &str, owner: Option<u64>) {
        self.clear(state);
        if !self.tty {
            Std::Err.write(&format!("{}\n", self.ctx(state).paint(&format!("{label}…"), "muted")));
            return;
        }
        state.animation = Some(Animation { label: label.to_string(), owner, frame: 0, last: Instant::now() });
        self.watch_resize(state, Some(Redraw::Animation));
        self.tick(state);
    }

    fn processing_result(&self, state: &mut State, label: &str, status: &str) {
        self.clear(state);
        self.append(state, Piece::Result { label: label.to_string(), failed: matches!(status, "failed" | "error") });
        self.watch_resize(state, None);
    }

    fn finish_status_for(&self, state: &mut State, message: &str, owner: Option<u64>, prefix: &str, outcome: Option<&str>) {
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

    fn draw(&self, state: &mut State, data: &Progress, prefix: String) {
        let piece = Piece::Progress { data: data.clone(), prefix: prefix.clone() };
        if state.region.is_some() {
            self.set_live(state, Some(piece));
            return;
        }
        let text = self.ctx(state).render(&piece);
        self.watch_resize(state, Some(Redraw::Progress(data.clone(), prefix)));
        self.render_live(state, &text);
    }

    fn update_title(&self, state: &State) {
        if state.started {
            let name = if state.name.is_empty() { String::new() } else { format!(" | {}", state.name) };
            self.title.set(self.tty, &format!("veo | {}{name}", state.phase));
        }
    }

    fn fail_step_for(&self, state: &mut State, owner: Option<u64>) {
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

    fn scoped_child(self: &Arc<Self>, index: usize, total: usize, title: &str, parent: &str) -> Arc<dyn Reporter> {
        Arc::new(Scoped {
            root: self.clone(),
            owner: self.owners.fetch_add(1, Ordering::SeqCst),
            prefix: format!("{parent}[{index}/{total}] "),
            child_name: Mutex::new(clean_text(title)),
            last_progress: Mutex::new(None),
        })
    }
}

fn ticker(reporter: Weak<TerminalReporter>) {
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
struct Scoped {
    root: Arc<TerminalReporter>,
    owner: u64,
    prefix: String,
    child_name: Mutex<String>,
    last_progress: Mutex<Option<Instant>>,
}

impl Scoped {
    fn child_name(&self) -> String {
        self.child_name.lock().unwrap_or_else(|poison| poison.into_inner()).clone()
    }

    fn log(&self, message: &str, quiet: bool) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(done: f64, total: Option<f64>, estimate: Option<f64>) -> Progress {
        Progress {
            status: Some("downloading".into()),
            downloaded_bytes: Some(done),
            total_bytes: total,
            total_bytes_estimate: estimate,
            speed: Some(1024.0 * 1024.0),
            eta: Some(65.0),
            stream: None,
        }
    }

    #[test]
    fn formats_known_totals() {
        let line = format_progress(&progress(512.0, Some(1024.0), None), &ProgressStyle { frame: 0, ..ProgressStyle::default() });
        assert_eq!(line, "[==========----------]  50%   1.0 MiB/s      512 B / 1.0 KiB  ETA 1:05");
    }

    #[test]
    fn marks_estimates_and_caps_them() {
        let line = format_progress(&progress(2048.0, None, Some(1024.0)), &ProgressStyle { frame: 0, ..ProgressStyle::default() });
        assert!(line.contains(" ~99%"), "{line}");
        assert!(line.contains("2.0 KiB / ~1.0 KiB"), "{line}");
    }

    #[test]
    fn finished_progress_reports_processing() {
        let mut data = progress(2048.0, Some(2048.0), None);
        data.status = Some("finished".into());
        assert_eq!(format_progress(&data, &ProgressStyle { prefix: "Video", ..ProgressStyle::default() }), "Video 2.0 KiB received; processing…");
    }

    #[test]
    fn unknown_total_animates_a_block() {
        let line = format_progress(&progress(10.0, None, None), &ProgressStyle { frame: 2, ..ProgressStyle::default() });
        assert!(line.starts_with("[--===---------------]   ?%"), "{line}");
    }

    #[test]
    fn narrow_terminals_wrap_statistics() {
        let line = format_progress(&progress(512.0, Some(1024.0), None), &ProgressStyle { columns: Some(30), frame: 0, ..ProgressStyle::default() });
        assert!(line.lines().all(|row| cells(row) <= 30), "{line}");
    }

    #[test]
    fn to_fixed_rounds_half_up() {
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(0.05, 1), "0.1");
        assert_eq!(bytes(Some(1536.0)), "1.5 KiB");
        assert_eq!(bytes(None), "?");
    }
}
