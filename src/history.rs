use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Map, Value};

use crate::error::{optional, Error, Result};
use crate::output::Out;
use crate::paths;
use crate::state;
use crate::terminal::Std;
use crate::utils::{clean_text, local_stamp};

/// `veo history` shows the most recent download attempts. Every finished item
/// is its own small file named after its millisecond timestamp, so parallel
/// runs never rewrite each other's records.
pub const HISTORY_LIMIT: usize = 5;
const SHOWN_FILES: usize = 5;
const MAX_FILES: usize = 200;
const STATUSES: [&str; 4] = ["saved", "skipped", "failed", "cancelled"];

pub fn history_help() -> String {
    format!(
        "veo history [--json] [--limit <n>] [--failed]

Show the last {HISTORY_LIMIT} download attempts by default, newest first, with title, status, media
type, date, URL and saved files. Saved, skipped, failed and cancelled items are
recorded; playlist entries and retried attempts count individually.
Use --limit to show 1-1000 attempts, and --failed to show only failed or cancelled
attempts. Use --json for scripting. Failed entries show their retry command while
the job file is available. History is kept in the per-user veo cache
and survives veo flush. Active downloads appear once they finish.
"
    )
}

pub fn history_root(root: &Path) -> PathBuf {
    paths::resolve(root).join("history")
}

fn valid(record: &Value) -> bool {
    let string_or_null = |key: &str| record.get(key).is_some_and(|value| value.is_null() || value.is_string());
    record.get("version").and_then(Value::as_i64) == Some(1)
        && record.get("at").and_then(Value::as_str).is_some_and(|at| chrono::DateTime::parse_from_rfc3339(at).is_ok())
        && record.get("url").and_then(Value::as_str).is_some_and(|url| !url.is_empty())
        && record.get("title").is_some_and(Value::is_string)
        && record.get("status").and_then(Value::as_str).is_some_and(|status| STATUSES.contains(&status))
        && record.get("media").and_then(Value::as_str).is_some_and(|media| media == "video" || media == "audio")
        && string_or_null("quality")
        && string_or_null("format")
        && string_or_null("error")
        && record.get("job").is_none_or(|job| job.is_null() || job.is_string())
        && record.get("runId").is_none_or(|id| id.is_null() || id.as_str().is_some_and(|id| re!(r"^[0-9a-z]{6}$").is_match(id)))
        && record.get("files").and_then(Value::as_array).is_some_and(|files| files.iter().all(|file| file.as_str().is_some_and(|file| !file.is_empty())))
}

/// One finished attempt, as the job runner reports it.
pub struct HistoryItem<'a> {
    pub url: &'a str,
    pub title: Option<&'a str>,
    pub status: &'a str,
    pub run_id: Option<&'a str>,
    pub job: Option<&'a str>,
    pub audio: bool,
    pub quality: &'a str,
    pub format: Option<&'a str>,
    pub files: &'a [String],
    pub error: Option<&'a str>,
    pub elapsed_ms: u64,
}

/// Writes history records with strictly increasing timestamps.
pub struct HistoryRecorder {
    directory: PathBuf,
    previous: Mutex<i64>,
}

impl HistoryRecorder {
    pub fn new(root: &Path) -> Self {
        HistoryRecorder { directory: history_root(root), previous: Mutex::new(0) }
    }

    /// Records never contain terminal escapes or grow without bound.
    pub fn record(&self, item: &HistoryItem) -> Result<()> {
        let at = {
            let mut previous = self.previous.lock().unwrap();
            let at = state::now_ms().max(*previous + 1);
            *previous = at;
            at
        };
        let text = |value: Option<&str>| value.map(clean_text).filter(|value| !value.is_empty());
        let mut record = Map::new();
        record.insert("version".into(), 1.into());
        record.insert("at".into(), Value::String(state::iso(chrono::DateTime::from_timestamp_millis(at).unwrap_or_default())));
        record.insert("url".into(), Value::String(clean_text(item.url)));
        record.insert("title".into(), Value::String(text(item.title).unwrap_or_else(|| clean_text(item.url))));
        record.insert("status".into(), Value::String(if STATUSES.contains(&item.status) { item.status } else { "failed" }.into()));
        record.insert("media".into(), Value::String(if item.audio { "audio" } else { "video" }.into()));
        record.insert("quality".into(), if item.audio { Value::Null } else { text(Some(item.quality)).map_or(Value::Null, Value::String) });
        record.insert("format".into(), text(item.format).or(if item.audio { Some("mp3".into()) } else { None }).map_or(Value::Null, Value::String));
        record
            .insert("files".into(), json!(item.files.iter().take(MAX_FILES).map(|file| clean_text(file)).filter(|file| !file.is_empty()).collect::<Vec<_>>()));
        record.insert("error".into(), text(item.error).map(|error| error.chars().take(400).collect::<String>()).map_or(Value::Null, Value::String));
        if let Some(job) = text(item.job) {
            record.insert("job".into(), Value::String(job));
        }
        if let Some(id) = text(item.run_id) {
            record.insert("runId".into(), Value::String(id));
        }
        record.insert("elapsedMs".into(), item.elapsed_ms.into());
        state::write_json(&self.directory.join(format!("{at}-{}.json", state::uuid())), &Value::Object(record))
    }
}

/// The newest `limit` records, newest first. Missing history is not an error.
pub fn read_history(root: &Path, limit: usize, failed: bool) -> Result<Vec<Value>> {
    let directory = history_root(root);
    let Some(info) = optional(fs::symlink_metadata(&directory))? else { return Ok(Vec::new()) };
    if !info.is_dir() || info.file_type().is_symlink() {
        return Err(Error::msg(format!("Invalid history directory: {}", directory.display())));
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type()?.is_file() && re!(r"^[0-9]{13}-[a-f0-9-]{36}\.json$").is_match(&name) {
            names.push(name);
        }
    }
    names.sort_by(|a, b| b[..13].cmp(&a[..13]));
    let mut records = Vec::new();
    for name in names {
        if records.len() >= limit {
            break;
        }
        let file = directory.join(&name);
        let record: Option<Value> = state::read_json(&file).unwrap_or(None);
        let record = record.filter(valid).ok_or_else(|| Error::msg(format!("Invalid history file: {}", file.display())))?;
        if failed && !matches!(record["status"].as_str(), Some("failed" | "cancelled")) {
            continue;
        }
        records.push(record);
    }
    Ok(records)
}

fn describe_duration(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms} ms");
    }
    let seconds = ms as f64 / 1000.0;
    if seconds < 60.0 {
        format!("{} s", crate::progress::to_fixed(seconds, 1))
    } else {
        format!("{}m {}s", (seconds / 60.0).floor(), (seconds % 60.0).floor())
    }
}

pub fn format_history(entries: &[Value], limit: usize, failed: bool) -> String {
    if entries.is_empty() {
        return if failed {
            "veo history\n\nNo failed or cancelled downloads recorded.\n".into()
        } else {
            "veo history\n\nNo downloads recorded yet. Download something with veo first.\n".into()
        };
    }
    let mut lines = vec![format!("veo history (last {limit}{}, newest first)", if failed { " failed/cancelled" } else { "" }), String::new()];
    for (index, entry) in entries.iter().enumerate() {
        let get = |key: &str| entry.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        lines.push(format!("{}. {}", index + 1, get("title")));
        let status = get("status");
        lines.push(format!("   Status: {status}{}", if status == "skipped" { " (already on disk)" } else { "" }));
        let run_id = get("runId");
        if !run_id.is_empty() {
            lines.push(format!("   Run ID: {run_id}  (veo inspect run {run_id})"));
        }
        let media = get("media");
        let detail = get(if media == "audio" { "format" } else { "quality" });
        lines.push(format!("   Media:  {}", if detail.is_empty() { media } else { format!("{media}, {detail}") }));
        let elapsed = entry.get("elapsedMs").and_then(Value::as_u64).unwrap_or(0);
        lines.push(format!("   Date:   {}{}", local_stamp(&get("at")), if elapsed > 0 { format!(" ({})", describe_duration(elapsed)) } else { String::new() }));
        lines.push(format!("   URL:    {}", get("url")));
        let error = get("error");
        if !error.is_empty() {
            lines.push(format!("   Error:  {error}"));
        }
        let job = get("job");
        if !job.is_empty() {
            lines.push(format!("   Retry:  veo --retry-failed {}", if run_id.is_empty() { format!("\"{job}\"") } else { run_id.clone() }));
        }
        let files: Vec<String> =
            entry.get("files").and_then(Value::as_array).map(|files| files.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
        for (position, file) in files.iter().take(SHOWN_FILES).enumerate() {
            lines.push(format!("{}{file}", if position > 0 { "           " } else { "   Saved:  " }));
        }
        if files.len() > SHOWN_FILES {
            lines.push(format!("           … and {} more file(s); use veo history --json for the full list.", files.len() - SHOWN_FILES));
        }
        if index != entries.len() - 1 {
            lines.push(String::new());
        }
    }
    format!("{}\n", lines.join("\n"))
}

pub fn history_main(args: &[String], root: &Path) -> Result<i32> {
    let stdout = Out::command(Std::Out, args);
    let args = crate::output::strip_color_flags(args);
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        stdout.write(&history_help());
        return Ok(0);
    }
    let (mut json, mut failed, mut limit) = (false, false, HISTORY_LIMIT);
    let usage = || Error::msg("Usage: veo history [--json] [--limit <n>] [--failed]");
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--json" if !json => json = true,
            "--failed" if !failed => failed = true,
            "--limit" => {
                index += 1;
                let value = args.get(index).map(String::as_str).unwrap_or("");
                if !re!(r"^[1-9][0-9]{0,3}$").is_match(value) || value.parse::<usize>().unwrap_or(usize::MAX) > 1000 {
                    return Err(Error::msg("--limit must be a whole number between 1 and 1000."));
                }
                limit = value.parse().unwrap_or(HISTORY_LIMIT);
            }
            _ => return Err(usage()),
        }
        index += 1;
    }
    let mut entries = read_history(root, limit, failed)?;
    if json {
        stdout.line(serde_json::to_string(&json!({ "count": entries.len(), "entries": entries }))?);
    } else {
        for entry in &mut entries {
            let usable = entry
                .get("job")
                .and_then(Value::as_str)
                .is_some_and(|job| fs::symlink_metadata(job).is_ok_and(|info| info.is_file() && !info.file_type().is_symlink()));
            if !usable && entry.get("job").is_some() {
                entry["job"] = Value::Null;
            }
        }
        stdout.write(&format_history(&entries, limit, failed));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_reads_newest_first() {
        let root = crate::fsutil::temp_dir(&std::env::temp_dir(), "veo-history-").unwrap();
        let recorder = HistoryRecorder::new(&root);
        for (index, status) in ["saved", "failed", "skipped"].iter().enumerate() {
            let files = vec![format!("/out/{index}.mp4")];
            recorder
                .record(&HistoryItem {
                    url: "https://example.com/v",
                    title: Some("\x1b[31mTitle"),
                    status,
                    run_id: Some("abc123"),
                    job: None,
                    audio: false,
                    quality: "best",
                    format: None,
                    files: &files,
                    error: None,
                    elapsed_ms: 1500,
                })
                .unwrap();
        }
        let entries = read_history(&root, 5, false).unwrap();
        assert_eq!(entries.iter().map(|entry| entry["status"].as_str().unwrap()).collect::<Vec<_>>(), ["skipped", "failed", "saved"]);
        assert_eq!(entries[0]["title"], "Title");
        assert_eq!(read_history(&root, 5, true).unwrap().len(), 1);
        let text = format_history(&entries, 5, false);
        assert!(text.starts_with("veo history (last 5, newest first)\n\n1. Title\n   Status: skipped (already on disk)\n   Run ID: abc123"), "{text}");
        assert!(text.contains("(1.5 s)"));
        let _ = fs::remove_dir_all(root);
    }
}
