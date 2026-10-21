use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::cancel::Cancel;
use crate::error::{optional, Error, Result};
use crate::output::Out;
use crate::paths;
use crate::run_archive::run_archive_file;
use crate::state;
use crate::terminal::Std;
use crate::utils::{clean_text, local_stamp};

/// Every veo run registers itself under the per-user cache's `runs` directory
/// while it works, so another terminal can list, inspect or stop it.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(15);
const ID_ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
const SHOWN_ITEMS: usize = 10;
const OUTPUT_WIDTH: usize = 42;

pub const RUNS_HELP: &str = "veo runs [id] [--json]

List active veo runs with their 6-character id, process id, start time and progress.
With an id, show one run in detail: URLs, media settings, output directory, job file
and the state of every item. A run appears while it works and disappears when it
finishes. Records of crashed runs are kept but marked stale.
Use --json for machine-readable run metadata and per-item progress.
Use veo stop <id> to stop one run.
";

pub const STOP_HELP: &str = "veo stop [id]

Ask one run, or every run when no id is given, to stop, and wait until it exits.
A stopped run keeps its partial data and its retry job, so the printed
veo --retry-failed command still works; veo flush removes those later.
Stale records of crashed runs are removed. Exit status is 1 when a run does not
stop in time.
";

pub fn runs_directory(root: &Path) -> PathBuf {
    paths::resolve(root).join("runs")
}

fn exists(file: &Path) -> Result<bool> {
    Ok(optional(fs::symlink_metadata(file))?.is_some())
}

fn text(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).map(clean_text).unwrap_or_default()
}

fn new_run_id() -> String {
    (0..6).map(|_| ID_ALPHABET[state::random_below(ID_ALPHABET.len() as u32) as usize] as char).collect()
}

/// Whether a process with this id still exists.
#[cfg(unix)]
pub fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks for existence and permission.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(windows)]
pub fn alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: the handle is checked and closed; the exit code is an out-parameter.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return std::io::Error::last_os_error().raw_os_error() == Some(5);
        }
        let mut code = 0u32;
        let running = GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE as u32;
        CloseHandle(handle);
        running
    }
}

#[cfg(not(any(unix, windows)))]
pub fn alive(_pid: u32) -> bool {
    true
}

#[derive(Clone, Debug)]
pub struct Run {
    pub record: Map<String, Value>,
    pub id: Option<String>,
    pub pid: u32,
    pub file: PathBuf,
    pub alive: bool,
    pub progress: Option<Value>,
}

impl Run {
    fn field(&self, key: &str) -> Option<&Value> {
        self.record.get(key)
    }

    pub fn job(&self) -> Option<String> {
        self.field("job").and_then(Value::as_str).filter(|job| !job.is_empty()).map(str::to_string)
    }

    fn started_at(&self) -> Option<String> {
        self.field("startedAt").and_then(Value::as_str).map(str::to_string)
    }

    fn label(&self) -> String {
        self.id.clone().unwrap_or_else(|| format!("PID {}", self.pid))
    }
}

fn run_files(root: &Path) -> Result<Vec<PathBuf>> {
    let directory = runs_directory(root);
    let Some(info) = optional(fs::symlink_metadata(&directory))? else { return Ok(Vec::new()) };
    if !info.is_dir() || info.file_type().is_symlink() {
        return Err(Error::msg(format!("Invalid active run directory: {}", directory.display())));
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

/// Damaged or foreign files are ignored, never fatal: a single stray file must
/// not block every later veo run.
fn read_records(root: &Path) -> Result<Vec<Run>> {
    let mut records = Vec::new();
    for file in run_files(root)? {
        let Some(Value::Object(record)) = state::read_json_lenient::<Value>(&file) else { continue };
        let version_ok = record.get("version").is_none() || record.get("version").and_then(Value::as_i64) == Some(1);
        let pid = record.get("pid").and_then(Value::as_u64).filter(|pid| *pid > 0 && *pid <= u64::from(u32::MAX));
        let (true, Some(pid)) = (version_ok, pid) else { continue };
        let id = record.get("id").and_then(Value::as_str).filter(|id| re!(r"^[0-9a-z]{6}$").is_match(id)).map(str::to_string);
        records.push(Run { record, id, pid: pid as u32, file, alive: false, progress: None });
    }
    Ok(records)
}

/// Every registered run, live ones first and then the most recently started.
pub fn list_runs(root: &Path) -> Result<Vec<Run>> {
    let mut runs = read_records(root)?;
    for run in &mut runs {
        run.alive = alive(run.pid);
    }
    runs.sort_by(|a, b| b.alive.cmp(&a.alive).then_with(|| b.started_at().unwrap_or_default().cmp(&a.started_at().unwrap_or_default())));
    Ok(runs)
}

/// This process's run registration: polls for stop requests and removes its
/// record when dropped or unregistered.
pub struct Registration {
    pub id: String,
    pub file: PathBuf,
    record: Mutex<Map<String, Value>>,
    stopped: Arc<AtomicBool>,
}

pub struct RunDetails<'a> {
    pub urls: &'a [String],
    pub output: Option<String>,
    pub audio: bool,
    pub quality: &'a str,
    pub format: Option<&'a str>,
    pub profile: Option<&'a str>,
    pub playlist: bool,
    pub job: Option<String>,
}

impl Registration {
    /// Merge what this run resolved, so other terminals can inspect it.
    pub fn describe(&self, details: &RunDetails) -> Result<()> {
        let mut record = self.record.lock().unwrap();
        let clean = |value: Option<&str>| value.map(clean_text).filter(|value| !value.is_empty()).map_or(Value::Null, Value::String);
        record.insert("urls".into(), json!(details.urls.iter().take(200).map(|url| clean_text(url)).collect::<Vec<_>>()));
        record.insert("output".into(), clean(details.output.as_deref()));
        record.insert("media".into(), Value::String(if details.audio { "audio" } else { "video" }.into()));
        record.insert("quality".into(), if details.audio { Value::Null } else { clean(Some(details.quality)) });
        record.insert("format".into(), clean(details.format));
        record.insert("profile".into(), clean(details.profile));
        record.insert("playlist".into(), details.playlist.into());
        record.insert("job".into(), clean(details.job.as_deref()));
        state::write_json(&self.file, &Value::Object(record.clone()))
    }

    pub fn unregister(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _ = fs::remove_file(&self.file);
        let _ = fs::remove_file(PathBuf::from(format!("{}.cancel", self.file.display())));
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.unregister();
    }
}

pub fn register_run(cancel: Cancel, root: &Path) -> Result<Registration> {
    let directory = runs_directory(root);
    fs::create_dir_all(&directory)?;
    let gate = paths::resolve(root).join(".flush");
    let busy = || Error::msg("veo flush is in progress. Try again after cleanup finishes.");
    if exists(&gate)? {
        return Err(busy());
    }
    let taken: Vec<String> = read_records(root)?.into_iter().filter_map(|run| run.id).collect();
    let mut id = None;
    for _ in 0..20 {
        let candidate = new_run_id();
        if !taken.contains(&candidate) && !exists(&run_archive_file(&candidate, root)?)? {
            id = Some(candidate);
            break;
        }
    }
    let id = id.ok_or_else(|| Error::msg("Could not allocate a free run id. Remove stale run records with veo stop or veo flush."))?;
    let file = directory.join(format!("{}.json", state::uuid()));
    let mut record = Map::new();
    record.insert("version".into(), 1.into());
    record.insert("id".into(), Value::String(id.clone()));
    record.insert("pid".into(), std::process::id().into());
    record.insert("startedAt".into(), Value::String(state::iso_now()));
    {
        let key = "urls";
        record.insert(key.into(), json!([]));
    }
    for key in ["output", "media", "quality", "format", "profile"] {
        record.insert(key.into(), Value::Null);
    }
    record.insert("playlist".into(), false.into());
    record.insert("job".into(), Value::Null);
    state::write_json(&file, &Value::Object(record.clone()))?;
    // A flush that started while this run registered must still win.
    if exists(&gate)? {
        let _ = fs::remove_file(&file);
        return Err(busy());
    }
    let stopped = Arc::new(AtomicBool::new(false));
    let request = PathBuf::from(format!("{}.cancel", file.display()));
    let flag = stopped.clone();
    std::thread::spawn(move || {
        while !flag.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(200));
            if fs::symlink_metadata(&request).is_ok() {
                cancel.cancel();
                return;
            }
        }
    });
    Ok(Registration { id, file, record: Mutex::new(record), stopped })
}

/// Per-item progress of a run, read from the job file it writes as it works.
fn read_progress(job: Option<&str>) -> Option<Value> {
    let job: Value = state::read_json_lenient(Path::new(job?))?;
    if job.get("version").and_then(Value::as_i64) != Some(1) {
        return None;
    }
    let items = job.get("items")?.as_array()?;
    let statuses = ["pending", "running", "saved", "skipped", "failed", "cancelled"];
    let mut counts: Map<String, Value> = Map::new();
    counts.insert("total".into(), items.len().into());
    for status in statuses {
        counts.insert(status.into(), 0.into());
    }
    let items: Vec<Value> = items
        .iter()
        .map(|item| {
            let status = item.get("status").and_then(Value::as_str).filter(|status| statuses.contains(status)).unwrap_or("pending");
            let count = counts[status].as_u64().unwrap_or(0) + 1;
            counts.insert(status.into(), count.into());
            let files: Vec<String> = item.get("files").and_then(Value::as_array).map(|files| files.iter().map(|file| text(Some(file))).filter(|file| !file.is_empty()).collect()).unwrap_or_default();
            let error = text(item.get("error"));
            json!({ "url": text(item.get("url")), "status": status, "files": files, "error": if error.is_empty() { Value::Null } else { Value::String(error) } })
        })
        .collect();
    Some(json!({ "items": items, "counts": counts }))
}

fn describe_runs(root: &Path) -> Result<Vec<Run>> {
    let mut runs = list_runs(root)?;
    for run in &mut runs {
        run.progress = read_progress(run.job().as_deref());
    }
    Ok(runs)
}

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

fn short_duration(ms: i64) -> String {
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

fn shorten(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= OUTPUT_WIDTH {
        value.to_string()
    } else {
        format!("…{}", chars[chars.len() - (OUTPUT_WIDTH - 1)..].iter().collect::<String>())
    }
}

// Records of older versions have no start time, so no uptime can be shown.
fn uptime(run: &Run) -> Option<String> {
    let started = chrono::DateTime::parse_from_rfc3339(&run.started_at()?).ok()?;
    Some(short_duration(state::now_ms() - started.timestamp_millis()))
}

fn describe_media(run: &Run) -> String {
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

fn normalize_id(value: &str) -> Result<String> {
    let id = clean_text(value).to_lowercase();
    if !re!(r"^[0-9a-z]{6}$").is_match(&id) {
        return Err(Error::msg(format!("Invalid run id \"{}\". Run ids have 6 characters and are listed by veo runs.", clean_text(value))));
    }
    Ok(id)
}

fn public_run(run: &Run) -> Value {
    let get = |key: &str| run.field(key).filter(|value| !value.is_null() && value.as_str() != Some("")).cloned().unwrap_or(Value::Null);
    json!({
        "id": run.id, "pid": run.pid, "state": if run.alive { "running" } else { "stale" }, "startedAt": get("startedAt"),
        "urls": run.field("urls").filter(|urls| urls.is_array()).cloned().unwrap_or(json!([])),
        "output": get("output"), "media": get("media"), "quality": get("quality"), "format": get("format"), "profile": get("profile"),
        "playlist": run.field("playlist") == Some(&Value::Bool(true)), "job": get("job"), "progress": run.progress.clone().unwrap_or(Value::Null),
    })
}

pub fn runs_main(args: &[String], root: &Path) -> Result<i32> {
    let stdout = Out::command(Std::Out, args);
    let mut args = crate::output::strip_color_flags(args);
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        stdout.write(RUNS_HELP);
        return Ok(0);
    }
    let json = args.iter().any(|arg| arg == "--json");
    args.retain(|arg| arg != "--json");
    if args.len() > 1 {
        return Err(Error::msg("Usage: veo runs [id] [--json]"));
    }
    let runs = describe_runs(root)?;
    let Some(requested) = args.first() else {
        if json {
            stdout.line(serde_json::to_string(&json!({ "count": runs.len(), "runs": runs.iter().map(public_run).collect::<Vec<_>>() }))?);
        } else {
            stdout.write(&format_runs(&runs));
        }
        return Ok(0);
    };
    let id = normalize_id(requested)?;
    let run = runs
        .iter()
        .find(|run| run.id.as_deref() == Some(id.as_str()))
        .ok_or_else(|| Error::msg(format!("No veo run with id {id} is active. List runs with veo runs.")))?;
    if json {
        stdout.line(serde_json::to_string(&public_run(run))?);
    } else {
        stdout.write(&format_run_details(run));
    }
    Ok(0)
}

/// A run answers a stop request by removing its own record, so a run counts
/// as gone as soon as its record disappears.
pub fn wait_for_exit(runs: &[Run], timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let pending = runs.iter().any(|run| alive(run.pid) && fs::symlink_metadata(&run.file).is_ok());
        if !pending {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn stop_main(args: &[String], root: &Path, timeout: Duration) -> Result<i32> {
    let stdout = Out::command(Std::Out, args);
    let args = crate::output::strip_color_flags(args);
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        stdout.write(STOP_HELP);
        return Ok(0);
    }
    if args.len() > 1 {
        return Err(Error::msg("Usage: veo stop [id]"));
    }
    let mut targets = list_runs(root)?;
    if let Some(requested) = args.first() {
        let id = normalize_id(requested)?;
        targets.retain(|run| run.id.as_deref() == Some(id.as_str()));
        if targets.is_empty() {
            return Err(Error::msg(format!("No veo run with id {id} is active. List runs with veo runs.")));
        }
    }
    let (live, stale): (Vec<Run>, Vec<Run>) = targets.into_iter().partition(|run| run.alive);
    for run in &live {
        fs::write(format!("{}.cancel", run.file.display()), "stop")?;
    }
    if !live.is_empty() {
        wait_for_exit(&live, timeout);
    }
    let (remaining, stopped): (Vec<Run>, Vec<Run>) = live.into_iter().partition(|run| fs::symlink_metadata(&run.file).is_ok() && alive(run.pid));
    for run in stale.iter().chain(stopped.iter()) {
        let _ = fs::remove_file(&run.file);
        let _ = fs::remove_file(format!("{}.cancel", run.file.display()));
    }
    let labels = |runs: &[Run]| runs.iter().map(Run::label).collect::<Vec<_>>().join(", ");
    let mut lines = vec!["veo stop".to_string(), String::new()];
    if !stopped.is_empty() {
        lines.push(format!("Stopped: {}", labels(&stopped)));
    }
    if !stale.is_empty() {
        lines.push(format!("Removed {} stale record(s): {}", stale.len(), labels(&stale)));
    }
    if !remaining.is_empty() {
        lines.push(format!("Still running: {}", labels(&remaining)));
    }
    if stopped.is_empty() && stale.is_empty() && remaining.is_empty() {
        lines.push("No veo runs were active.".into());
    }
    if !stopped.is_empty() {
        lines.push(String::new());
        lines.push("Partial data and retry jobs are kept; veo flush removes them.".into());
    }
    stdout.line(lines.join("\n"));
    Ok(if remaining.is_empty() { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_lists_and_unregisters() {
        let root = crate::fsutil::temp_dir(&std::env::temp_dir(), "veo-runs-").unwrap();
        let registration = register_run(Cancel::new(), &root).unwrap();
        let urls = vec!["https://example.com/v".to_string()];
        registration
            .describe(&RunDetails {
                urls: &urls,
                output: Some("/out".into()),
                audio: false,
                quality: "720p",
                format: None,
                profile: None,
                playlist: false,
                job: None,
            })
            .unwrap();
        let runs = list_runs(&root).unwrap();
        assert_eq!(runs.len(), 1);
        assert!(runs[0].alive);
        assert_eq!(runs[0].id.as_deref(), Some(registration.id.as_str()));
        assert!(format_runs(&runs).contains("running"));
        assert!(format_run_details(&runs[0]).contains("Media:   video, 720p"));
        drop(registration);
        assert!(list_runs(&root).unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn summarizes_progress() {
        let progress = json!({ "counts": { "total": 3, "pending": 1, "running": 1, "saved": 1, "skipped": 0, "failed": 0, "cancelled": 0 } });
        assert_eq!(summarize_progress(Some(&progress)), "1/3 done, 1 running, 1 pending");
        assert_eq!(summarize_progress(None), "starting");
    }
}
