//! Text and JSON views of active runs.

use serde_json::{json, Value};

use crate::state;
use crate::utils::local_stamp;

use super::*;

pub fn summarize_progress(progress: Option<&Value>) -> String {
    let count = |key: &str| progress.and_then(|progress| progress.pointer(&format!("/counts/{key}"))).and_then(Value::as_u64).unwrap_or(0);
    let total = count("total");
    if total == 0 {
        return "starting".into();
    }
    let mut parts = Vec::new();
    let done = count("saved") + count("skipped");
    if done > 0 {
        parts.push(format!("{done}/{total} done"));
    }
    for (key, label) in [("running", "running"), ("failed", "failed"), ("cancelled", "cancelled")] {
        if count(key) > 0 {
            parts.push(format!("{} {label}", count(key)));
        }
    }
    if count("pending") > 0 {
        parts.push(if parts.is_empty() { format!("{total} pending") } else { format!("{} pending", count("pending")) });
    }
    parts.join(", ")
}

pub(super) fn short_duration(ms: i64) -> String {
    if ms < 0 {
        return "0s".into();
    }
    let seconds = ms / 1000;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3600, seconds % 3600 / 60)
    }
}

pub(super) fn shorten(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= OUTPUT_WIDTH {
        value.to_string()
    } else {
        format!("…{}", chars[chars.len() - (OUTPUT_WIDTH - 1)..].iter().collect::<String>())
    }
}

// Records of older versions have no start time, so no uptime can be shown.
pub(super) fn uptime(run: &Run) -> Option<String> {
    let started = chrono::DateTime::parse_from_rfc3339(&run.started_at()?).ok()?;
    Some(short_duration(state::now_ms() - started.timestamp_millis()))
}

pub(super) fn describe_media(run: &Run) -> String {
    let media = text(run.field("media"));
    let detail = text(run.field(if media == "audio" { "format" } else { "quality" }));
    format!(
        "{}{}{}",
        if media.is_empty() { "not resolved yet".to_string() } else { media },
        if detail.is_empty() { String::new() } else { format!(", {detail}") },
        if run.field("playlist") == Some(&Value::Bool(true)) { ", playlist" } else { "" }
    )
}

pub fn format_runs(runs: &[Run]) -> String {
    if runs.is_empty() {
        return "veo runs\n\nNo veo runs are active. Start a download to see it here.\n".into();
    }
    let rows: Vec<[String; 6]> = runs
        .iter()
        .map(|run| {
            let state = if run.alive { format!("running{}", uptime(run).map(|value| format!(" {value}")).unwrap_or_default()) } else { "stale".into() };
            let output = text(run.field("output"));
            [
                run.id.clone().unwrap_or_else(|| "-".into()),
                run.pid.to_string(),
                state,
                local_stamp(&run.started_at().unwrap_or_default()),
                summarize_progress(run.progress.as_ref()),
                if output.is_empty() { "-".into() } else { shorten(&output) },
            ]
        })
        .collect();
    let header = ["ID", "PID", "STATE", "STARTED", "ITEMS", "OUTPUT"].map(str::to_string);
    let widths: Vec<usize> = (0..6).map(|index| rows.iter().map(|row| row[index].chars().count()).chain([header[index].len()]).max().unwrap_or(0)).collect();
    let line = |cells: &[String; 6]| {
        cells.iter().enumerate().map(|(index, cell)| format!("{cell:<width$}", width = widths[index])).collect::<Vec<_>>().join("  ").trim_end().to_string()
    };
    let mut lines = vec!["veo runs".to_string(), String::new(), line(&header)];
    lines.extend(rows.iter().map(line));
    lines.push(String::new());
    lines.push("Details: veo runs <id>    Stop: veo stop [id]".into());
    format!("{}\n", lines.join("\n"))
}

pub fn format_run_details(run: &Run) -> String {
    let mut lines = vec![format!("veo runs {}", run.id.as_deref().unwrap_or("-")), String::new()];
    let running = uptime(run);
    lines.push(format!("State:   {}", if run.alive { format!("running (PID {})", run.pid) } else { format!("stale (PID {} is gone)", run.pid) }));
    lines.push(format!(
        "Started: {}{}",
        local_stamp(&run.started_at().unwrap_or_default()),
        match (&running, run.alive) {
            (Some(value), true) => format!(" ({value} ago)"),
            _ => String::new(),
        }
    ));
    lines.push(format!("Media:   {}", describe_media(run)));
    let or = |value: String, fallback: &str| if value.is_empty() { fallback.to_string() } else { value };
    lines.push(format!("Profile: {}", or(text(run.field("profile")), "global (no profile)")));
    lines.push(format!("Output:  {}", or(text(run.field("output")), "not resolved yet")));
    lines.push(format!("Job:     {}", or(text(run.field("job")), "not created yet")));
    let urls: Vec<String> = run
        .field("urls")
        .and_then(Value::as_array)
        .map(|urls| urls.iter().map(|url| text(Some(url))).filter(|url| !url.is_empty()).collect())
        .unwrap_or_default();
    lines.push(format!("URLs:    {}", if urls.is_empty() { "not resolved yet".into() } else { urls.len().to_string() }));
    for (index, url) in urls.iter().take(SHOWN_ITEMS).enumerate() {
        lines.push(format!("  {}. {url}", index + 1));
    }
    if urls.len() > SHOWN_ITEMS {
        lines.push(format!("  … and {} more", urls.len() - SHOWN_ITEMS));
    }
    let items = run.progress.as_ref().and_then(|progress| progress["items"].as_array()).cloned().unwrap_or_default();
    if !items.is_empty() {
        lines.push(format!("Items:   {}", summarize_progress(run.progress.as_ref())));
        for (index, item) in items.iter().take(SHOWN_ITEMS).enumerate() {
            let status = item["status"].as_str().unwrap_or("pending");
            let files = item["files"].as_array().cloned().unwrap_or_default();
            let detail = if status == "saved" {
                match files.first().and_then(Value::as_str) {
                    Some(first) => format!("  {first}{}", if files.len() > 1 { format!(" (+{} file(s))", files.len() - 1) } else { String::new() }),
                    None => String::new(),
                }
            } else {
                format!("  {}", item["url"].as_str().unwrap_or(""))
            };
            lines.push(format!("  {:>2}. {status}{detail}", index + 1));
            if let Some(error) = item["error"].as_str() {
                lines.push(format!("      {error}"));
            }
        }
        if items.len() > SHOWN_ITEMS {
            lines.push(format!("  … and {} more item(s)", items.len() - SHOWN_ITEMS));
        }
    }
    lines.push(String::new());
    lines.push(match (&run.id, run.alive) {
        (Some(id), true) => format!("Stop:    veo stop {id}"),
        _ => "Cleanup: veo stop  (removes stale records)".into(),
    });
    format!("{}\n", lines.join("\n"))
}

pub(super) fn public_run(run: &Run) -> Value {
    let get = |key: &str| run.field(key).filter(|value| !value.is_null() && value.as_str() != Some("")).cloned().unwrap_or(Value::Null);
    json!({
        "id": run.id, "pid": run.pid, "state": if run.alive { "running" } else { "stale" }, "startedAt": get("startedAt"),
        "urls": run.field("urls").filter(|urls| urls.is_array()).cloned().unwrap_or(json!([])),
        "output": get("output"), "media": get("media"), "quality": get("quality"), "format": get("format"), "profile": get("profile"),
        "playlist": run.field("playlist") == Some(&Value::Bool(true)), "job": get("job"), "progress": run.progress.clone().unwrap_or(Value::Null),
    })
}
