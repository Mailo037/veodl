//! Progress data from yt-dlp and its one-line rendering.

use serde::Deserialize;

use crate::terminal::{cells, fit, wrap};
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

pub(super) fn processing_label(processor: &str) -> &'static str {
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

pub(super) fn ended(status: Option<&str>) -> bool {
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

pub(super) fn pad_start(text: &str, width: usize) -> String {
    format!("{}{text}", " ".repeat(width.saturating_sub(text.chars().count())))
}

pub(super) fn pad_end(text: &str, width: usize) -> String {
    format!("{text}{}", " ".repeat(width.saturating_sub(text.chars().count())))
}

pub(super) const BAR_WIDTH: usize = 20;

pub(super) fn frame_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|time| time.as_millis() as u64 / 120).unwrap_or(0)
}

// Fixed-width fields keep the bar and statistics from jumping between updates.
pub(super) fn progress_bar(percent: Option<f64>, size: usize, unicode: bool, paint: &dyn Fn(&str, &str) -> String, frame: u64) -> String {
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

pub(super) fn no_paint(text: &str, _: &str) -> String {
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
