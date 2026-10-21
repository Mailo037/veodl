use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Map, Value};

use crate::error::{optional, Error, Result};
use crate::output::Out;
use crate::state;
use crate::terminal::Std;

const FIELDS: [&str; 6] = ["videos", "audio", "failed", "skipped", "cancelled", "elapsedMs"];

pub const STATS_HELP: &str = "veo stats [--json]\n\nShow recorded download totals. Time includes preparation, processing and saving,\nincluding failed attempts. Parallel run times are added together.\nStats start with this version; old downloads are not imported.\nUse veo flush --stats to clear temporary data and reset statistics.\n";

#[derive(Clone, Copy, Default)]
pub struct Delta {
    pub videos: u64,
    pub audio: u64,
    pub failed: u64,
    pub skipped: u64,
    pub cancelled: u64,
    pub elapsed_ms: u64,
}

/// Per-process totals in their own file, so parallel runs never conflict.
pub struct StatsRecorder {
    file: PathBuf,
    totals: Mutex<Map<String, Value>>,
}

impl StatsRecorder {
    pub fn new(root: &Path) -> Self {
        let mut totals = Map::new();
        totals.insert("version".into(), 1.into());
        totals.insert("since".into(), Value::String(state::iso_now()));
        for key in FIELDS {
            totals.insert(key.into(), 0.into());
        }
        StatsRecorder { file: root.join("stats").join(format!("{}.json", state::uuid())), totals: Mutex::new(totals) }
    }

    pub fn record(&self, delta: Delta) -> Result<()> {
        let mut totals = self.totals.lock().unwrap();
        for (key, value) in FIELDS.iter().zip([delta.videos, delta.audio, delta.failed, delta.skipped, delta.cancelled, delta.elapsed_ms]) {
            let current = totals[*key].as_u64().unwrap_or(0);
            totals.insert((*key).into(), (current + value).into());
        }
        state::write_json(&self.file, &Value::Object(totals.clone()))
    }
}

fn stat_files(root: &Path) -> Result<Vec<PathBuf>> {
    let directory = crate::paths::resolve(root).join("stats");
    let Some(info) = optional(fs::symlink_metadata(&directory))? else { return Ok(Vec::new()) };
    if !info.is_dir() || info.file_type().is_symlink() {
        return Err(Error::msg(format!("Invalid statistics directory: {}", directory.display())));
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type()?.is_file() && re!(r"^[a-f0-9-]{36}\.json$").is_match(&name) {
            files.push(directory.join(name));
        }
    }
    Ok(files)
}

pub fn read_stats(root: &Path) -> Result<Map<String, Value>> {
    let mut since: Option<String> = None;
    let mut sums = [0f64; 6];
    for file in stat_files(root)? {
        let Some(value) = state::read_json::<Value>(&file)? else { continue };
        let valid_since = value.get("since").and_then(Value::as_str).filter(|since| chrono::DateTime::parse_from_rfc3339(since).is_ok());
        let numbers: Option<Vec<f64>> =
            FIELDS.iter().map(|key| value.get(*key).and_then(Value::as_f64).filter(|number| number.is_finite() && *number >= 0.0)).collect();
        let (Some(1), Some(valid_since), Some(numbers)) = (value.get("version").and_then(Value::as_i64), valid_since, numbers) else {
            return Err(Error::msg(format!("Invalid statistics file: {}", file.display())));
        };
        if since.as_deref().is_none_or(|current| valid_since < current) {
            since = Some(valid_since.to_string());
        }
        for (sum, number) in sums.iter_mut().zip(numbers) {
            *sum += number;
        }
    }
    let mut result = Map::new();
    result.insert("since".into(), since.map_or(Value::Null, Value::String));
    for (key, sum) in FIELDS.iter().zip(sums) {
        result.insert((*key).into(), crate::downloader::number(sum));
    }
    Ok(result)
}

/// Called by flush only after registered runs stopped and while its gate is held.
pub fn reset_stats(root: &Path) -> Result<()> {
    for file in stat_files(root)? {
        crate::fsutil::remove_file(&file)?;
    }
    Ok(())
}

pub fn stats_main(args: &[String], root: &Path) -> Result<i32> {
    let stdout = Out::command(Std::Out, args);
    let args = crate::output::strip_color_flags(args);
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        stdout.write(STATS_HELP);
        return Ok(0);
    }
    if !args.is_empty() && (args.len() != 1 || args[0] != "--json") {
        return Err(Error::msg("Usage: veo stats [--json]"));
    }
    let stats = read_stats(root)?;
    if args.first().is_some_and(|arg| arg == "--json") {
        stdout.line(serde_json::to_string(&stats)?);
        return Ok(0);
    }
    let get = |key: &str| stats[key].as_f64().unwrap_or(0.0);
    let seconds = (get("elapsedMs") / 1000.0).floor() as u64;
    stdout.write(&format!(
        "veo stats\n\nTracking since: {}\nVideos saved:   {}\nAudio saved:    {}\nTotal failures: {}\nSkipped:        {}\nCancelled:      {}\nDownload time:  {}h {}m {}s\n\nTime includes preparation, processing and saving. Active requests appear when finished.\n",
        stats["since"].as_str().unwrap_or("no downloads yet"),
        stats["videos"], stats["audio"], stats["failed"], stats["skipped"], stats["cancelled"],
        seconds / 3600, (seconds / 60) % 60, seconds % 60
    ));
    let _ = json!(null);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_recorders() {
        let root = crate::fsutil::temp_dir(&std::env::temp_dir(), "veo-stats-").unwrap();
        for videos in [1, 2] {
            StatsRecorder::new(&root).record(Delta { videos, elapsed_ms: 1000, ..Delta::default() }).unwrap();
        }
        let stats = read_stats(&root).unwrap();
        assert_eq!(stats["videos"], json!(3));
        assert_eq!(stats["elapsedMs"], json!(2000));
        reset_stats(&root).unwrap();
        assert_eq!(read_stats(&root).unwrap()["videos"], json!(0));
        let _ = fs::remove_dir_all(root);
    }
}
