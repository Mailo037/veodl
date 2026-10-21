use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{json, Map, Value};

use crate::cancel::Cancel;
use crate::downloader::{DownloadResult, EntryOutcome};
use crate::error::{optional, Error, Result};
use crate::execution::{format_timings, run_pool, Adaptive};
use crate::history::{HistoryItem, HistoryRecorder};
use crate::options::Options;
use crate::output::Out;
use crate::path_links::folder_link;
use crate::paths;
use crate::progress::Reporter;
use crate::run_archive::{self, file_fingerprint, is_media_file, Fingerprint, JobItemView};
use crate::state;
use crate::stats::{Delta, StatsRecorder};
use crate::terminal::{self, path_rows, style, word_wrap};
use crate::utils::{clean_text, readable_error, validate_url};

const JOB_NAME: &str = r"^[0-9]{13}-[a-f0-9-]{36}\.json$";

fn read_job(file: &Path) -> Result<Option<Value>> {
    match state::read_json::<Value>(file) {
        Ok(value) => Ok(value),
        Err(error) if error.is_code("EJSON") => Ok(None),
        Err(error) => Err(error),
    }
}

/// Options of every failed or unfinished item of a retry job (by run ID or file).
pub fn retry_options(file: &str, root: &Path) -> Result<Vec<Map<String, Value>>> {
    let file = if run_archive::valid_id(file) { retry_job_for_run(file, root)? } else { paths::resolve(file) };
    let job: Option<Value> = state::read_json(&file)?;
    let job = job
        .filter(|job| job.get("version").and_then(Value::as_i64) == Some(1) && job.get("items").is_some_and(Value::is_array))
        .ok_or_else(|| Error::msg("Invalid retry job file."))?;
    let pending: Vec<&Value> =
        job["items"].as_array().unwrap().iter().filter(|item| !matches!(item.get("status").and_then(Value::as_str), Some("saved" | "skipped"))).collect();
    if pending.is_empty() {
        return Err(Error::msg("This job has no failed or unfinished downloads."));
    }
    pending
        .into_iter()
        .map(|item| {
            let url = item.get("url").and_then(Value::as_str).unwrap_or("");
            validate_url(url)?;
            let mut options = job.get("options").and_then(Value::as_object).cloned().unwrap_or_default();
            for (key, value) in item.get("options").and_then(Value::as_object).cloned().unwrap_or_default() {
                options.insert(key, value);
            }
            options.insert("url".into(), Value::String(url.into()));
            options.insert("resume".into(), true.into());
            // After interruption, retry the original selection; resume history skips finished entries.
            let failures: Vec<String> = item
                .get("entries")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|entry| entry.get("status").and_then(Value::as_str) == Some("failed"))
                .filter_map(|entry| entry.get("index").map(|index| index.to_string()))
                .collect();
            if item.get("status").and_then(Value::as_str) == Some("failed")
                && !failures.is_empty()
                && item.get("finished").is_some_and(|finished| finished == &Value::Bool(true))
            {
                options.insert("playlistItems".into(), Value::String(failures.join(",")));
            }
            Ok(options)
        })
        .collect()
}

fn job_names(directory: &Path) -> Result<Vec<String>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| re!(JOB_NAME).is_match(name))
        .collect();
    names.sort_by(|a, b| b.cmp(a));
    Ok(names)
}

/// Resolve a persistent run ID through its original retry job.
fn retry_job_for_run(id: &str, root: &Path) -> Result<PathBuf> {
    if crate::runs::list_runs(root)?.iter().any(|run| run.id.as_deref() == Some(id) && run.alive) {
        return Err(Error::msg(format!("Run {id} is still active. Stop it before retrying.")));
    }
    let directory = paths::resolve(root).join("jobs");
    for name in job_names(&directory)? {
        let file = directory.join(name);
        if read_job(&file)?.is_some_and(|job| job.get("runId").and_then(Value::as_str) == Some(id)) {
            return Ok(file);
        }
    }
    Err(Error::msg(format!("No retry job found for run {id}. Retry jobs may have been removed by veo flush.")))
}

/// Location of one run's job file, created before the first download starts.
pub fn job_file_path(root: &Path) -> PathBuf {
    root.join("jobs").join(format!("{}-{}.json", state::now_ms(), state::uuid()))
}

/// The newest retryable job, skipping finished jobs and active runs.
pub fn latest_failed_job(root: &Path) -> Result<PathBuf> {
    let directory = paths::resolve(root).join("jobs");
    let info = optional(fs::symlink_metadata(&directory))?.ok_or_else(|| Error::msg("No retry jobs found. Run a download first."))?;
    if !info.is_dir() || info.file_type().is_symlink() {
        return Err(Error::msg(format!("Invalid retry job directory: {}", directory.display())));
    }
    let active: HashSet<PathBuf> = crate::runs::list_runs(root)?.into_iter().filter(|run| run.alive).filter_map(|run| run.job()).map(paths::resolve).collect();
    for name in job_names(&directory)? {
        let file = directory.join(name);
        if active.contains(&file) {
            continue;
        }
        let Some(job) = read_job(&file)? else { continue };
        let items = job.get("items").and_then(Value::as_array);
        let valid = job.get("version").and_then(Value::as_i64) == Some(1)
            && job.get("options").is_some_and(Value::is_object)
            && items.is_some_and(|items| {
                items.iter().all(|item| item.get("url").is_some_and(Value::is_string) && item.get("status").is_some_and(Value::is_string))
            });
        if valid && items.is_some_and(|items| items.iter().any(|item| !matches!(item["status"].as_str(), Some("saved" | "skipped")))) {
            return Ok(file);
        }
    }
    Err(Error::msg("No failed or unfinished retry job found."))
}

/// Downloads one request with the given reporter, entry callback and staging root.
pub type DownloadFn<'a> =
    dyn Fn(&Options, Arc<dyn Reporter>, &Adaptive, &(dyn Fn(&EntryOutcome) -> Result<()> + Sync), Option<&Path>) -> Result<DownloadResult> + Sync + 'a;

pub struct JobContext<'a> {
    pub download: &'a DownloadFn<'a>,
    pub reporter: Arc<dyn Reporter>,
    pub stdout: Out,
    pub stderr: Out,
    pub cancel: &'a Cancel,
    pub open_file: &'a (dyn Fn(&Path) -> Result<()> + Sync),
    pub items: Option<Vec<Options>>,
    pub job_file: Option<PathBuf>,
    pub run_id: Option<String>,
    pub stats: Option<StatsRecorder>,
    pub history: Option<HistoryRecorder>,
    pub archive_root: PathBuf,
}

fn insert_opt(map: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    match value {
        Some(value) => map.insert(key.into(), value),
        None => map.remove(key),
    };
}

/// Run every request of one invocation, recording progress in the job file,
/// printing results, and returning the exit code (0, 1 or 130).
pub fn run_job(options: &Options, mut context: JobContext) -> Result<i32> {
    if options.incognito {
        context.run_id = None;
        context.stats = None;
        context.history = None;
    }
    let concurrency = options.concurrent_downloads;
    if !(1..=4).contains(&concurrency) {
        return Err(Error::msg("Concurrent downloads must be between 1 and 4."));
    }
    let file = context.job_file.clone().unwrap_or_else(|| job_file_path(&paths::cache_base()));
    let requests: Vec<Options> = context.items.clone().unwrap_or_else(|| options.urls.iter().map(|url| options.with_url(url)).collect());
    let mut job = Map::new();
    job.insert("version".into(), 1.into());
    if let Some(id) = &context.run_id {
        job.insert("runId".into(), Value::String(id.clone()));
    }
    job.insert("startedAt".into(), Value::String(state::iso_now()));
    job.insert("options".into(), Value::Object(options.public()));
    job.insert(
        "items".into(),
        Value::Array(requests.iter().map(|request| json!({ "url": request.url, "status": "pending", "entries": [], "options": request.public() })).collect()),
    );
    if !options.incognito {
        crate::download_cache::cleanup(&crate::download_cache::root(), state::now_ms())?;
    }
    let private_root = if options.incognito { Some(crate::fsutil::temp_dir(&std::env::temp_dir(), "veo-incognito-")?) } else { None };
    let job = Mutex::new(job);
    let persist = || -> Result<()> {
        if options.incognito {
            return Ok(());
        }
        let job = job.lock().unwrap();
        state::write_json(&file, &Value::Object(job.clone()))
    };
    persist()?;
    let fingerprints: Mutex<Vec<BTreeMap<String, Option<Fingerprint>>>> = Mutex::new(vec![BTreeMap::new(); requests.len()]);
    let active_targets: Mutex<HashMap<String, Arc<Mutex<()>>>> = Mutex::new(HashMap::new());
    let totals = Mutex::new((0u64, 0u64, 0u64));
    let adaptive = Adaptive::default();
    let color = options.color;
    let context = &context;
    let update_item = |index: usize, change: &dyn Fn(&mut Map<String, Value>)| {
        let mut job = job.lock().unwrap();
        if let Some(Value::Object(item)) = job.get_mut("items").and_then(|items| items.get_mut(index)) {
            change(item);
        }
    };
    let pool = run_pool(&requests, &|| adaptive.reduced(concurrency), context.cancel, &|request: &Options, index: usize| {
        let target_key = serde_json::to_string(&json!([request.url, paths::display(paths::resolve(&request.output))])).unwrap_or_default();
        let gate = active_targets.lock().unwrap().entry(target_key).or_insert_with(|| Arc::new(Mutex::new(()))).clone();
        let _serial = gate.lock().unwrap_or_else(|poison| poison.into_inner());
        context.cancel.check()?;
        let (mut saved, mut skipped, mut failed) = (0u64, 0u64, 0u64);
        let item_reporter: Arc<dyn Reporter> = if concurrency > 1 && requests.len() > 1 {
            context.reporter.scoped(index + 1, requests.len(), &request.url).unwrap_or_else(|| context.reporter.clone())
        } else {
            context.reporter.clone()
        };
        let capture = |files: &[String]| {
            if context.run_id.is_none() {
                return;
            }
            for target in files {
                if !is_media_file(target) || fingerprints.lock().unwrap()[index].contains_key(target) {
                    continue;
                }
                let fingerprint = file_fingerprint(Path::new(target), None).ok().flatten();
                fingerprints.lock().unwrap()[index].insert(target.clone(), fingerprint.clone());
                update_item(index, &|item| {
                    let map = item.entry("fingerprints").or_insert_with(|| json!({}));
                    map[target.as_str()] = serde_json::to_value(&fingerprint).unwrap_or(Value::Null);
                });
            }
        };
        let started = Instant::now();
        let published: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
        let opened = Mutex::new(false);
        let publish = |files: &[String]| {
            if !options.json {
                for target in files {
                    if published.lock().unwrap().insert(target.clone()) {
                        let rows = path_rows(context.stdout.tty, "Saved: ", target);
                        let text = rows
                            .iter()
                            .enumerate()
                            .map(|(line, row)| {
                                style(
                                    context.stdout.tty,
                                    &format!("{}{}", if line > 0 { "       " } else { "Saved: " }, folder_link(context.stdout.tty, Path::new(target), row)),
                                    "success",
                                    color,
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        context.stdout.write(&format!("{text}\n"));
                    }
                }
            } else {
                published.lock().unwrap().extend(files.iter().cloned());
            }
            let mut opened = opened.lock().unwrap();
            if request.open && !*opened && !files.is_empty() {
                *opened = true;
                if let Err(error) = (context.open_file)(Path::new(&files[0])) {
                    context.stderr.write(&format!("veo: File saved, but could not launch the default app: {}\n", readable_error(&error)));
                }
            }
        };
        if Arc::ptr_eq(&item_reporter, &context.reporter) {
            context.reporter.item(index + 1, requests.len(), &request.url);
        }
        update_item(index, &|item| {
            item.insert("status".into(), "running".into());
        });
        persist()?;
        let entry_counts = Mutex::new((0u64, 0u64, 0u64, 0usize));
        let on_entry = |entry: &EntryOutcome| -> Result<()> {
            capture(&entry.files);
            update_item(index, &|item| {
                if let Some(Value::Array(entries)) = item.get_mut("entries") {
                    entries.push(entry.to_json());
                }
            });
            persist()?;
            let mut counts = entry_counts.lock().unwrap();
            counts.3 += 1;
            match entry.status.as_str() {
                "saved" => counts.0 += 1,
                "skipped" => counts.1 += 1,
                "failed" => counts.2 += 1,
                _ => {}
            }
            drop(counts);
            publish(&entry.files);
            Ok(())
        };
        let outcome = (context.download)(request, item_reporter.clone(), &adaptive, &on_entry, private_root.as_deref());
        let (entry_saved, entry_skipped, entry_failed, entry_count) = *entry_counts.lock().unwrap();
        saved += entry_saved;
        skipped += entry_skipped;
        failed += entry_failed;
        let mut title: Option<String> = None;
        let (status, error_text, item_files) = match outcome {
            Ok(result) => {
                capture(&result.files);
                let status = if result.status.is_empty() { "saved".to_string() } else { result.status.clone() };
                update_item(index, &|item| {
                    item.insert("status".into(), Value::String(status.clone()));
                    item.insert("files".into(), json!(result.files));
                    insert_opt(item, "timings", result.timings.clone().map(Value::Object));
                    insert_opt(item, "entryTimings", (!result.entry_timings.is_empty()).then(|| Value::Array(result.entry_timings.clone())));
                    item.insert("finished".into(), true.into());
                });
                title = result.title.clone();
                if entry_count == 0 {
                    saved += result.saved;
                    skipped += result.skipped;
                    failed += result.failures.as_ref().map_or(if status == "failed" { 1 } else { 0 }, |failures| failures.len() as u64);
                }
                if options.json {
                    let mut line = result.to_json();
                    line.insert("status".into(), Value::String(status.clone()));
                    line.insert("profile".into(), request.profile_value());
                    if let Some(id) = &context.run_id {
                        line.insert("runId".into(), Value::String(id.clone()));
                    }
                    context.stdout.write(&format!("{}\n", serde_json::to_string(&line)?));
                }
                publish(&result.files);
                (status, None, result.files)
            }
            Err(error) => {
                let status = if context.cancel.is_cancelled() || error.is_cancelled() { "cancelled" } else { "failed" }.to_string();
                let message = readable_error(&error);
                if status != "cancelled" {
                    failed += 1;
                }
                let mut files: Vec<String> = Vec::new();
                {
                    let job = job.lock().unwrap();
                    for entry in job["items"][index]["entries"].as_array().into_iter().flatten() {
                        for file in entry["files"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                            if !files.iter().any(|known| known == file) {
                                files.push(file.to_string());
                            }
                        }
                    }
                }
                for file in &error.files {
                    if !files.contains(file) {
                        files.push(file.clone());
                    }
                }
                update_item(index, &|item| {
                    item.insert("status".into(), Value::String(status.clone()));
                    item.insert("error".into(), Value::String(message.clone()));
                    item.insert("files".into(), json!(files));
                });
                if options.json {
                    let mut line = Map::new();
                    line.insert("url".into(), Value::String(request.url.clone()));
                    line.insert("status".into(), Value::String(status.clone()));
                    line.insert("profile".into(), request.profile_value());
                    line.insert("error".into(), Value::String(message.clone()));
                    line.insert("files".into(), json!(files));
                    if let Some(id) = &context.run_id {
                        line.insert("runId".into(), Value::String(id.clone()));
                    }
                    context.stdout.write(&format!("{}\n", serde_json::to_string(&line)?));
                } else {
                    // Errors wrap instead of being shortened; the cause is usually at the end.
                    let text = clean_text(&format!("veo: {message} ({})", request.url));
                    let rows: Vec<String> = if context.stderr.tty {
                        word_wrap(&text, Some(terminal::columns().saturating_sub(1).max(20))).split('\n').map(str::to_string).collect()
                    } else {
                        vec![text]
                    };
                    context
                        .stderr
                        .write(&format!("{}\n", rows.iter().map(|row| style(context.stderr.tty, row, "error", color)).collect::<Vec<_>>().join("\n")));
                }
                if error.verification_failed && !options.json {
                    for file in &files {
                        context.stderr.write(&format!("Saved file retained: {}\n", clean_text(file)));
                    }
                }
                (status, Some(message), files)
            }
        };
        let elapsed_ms = started.elapsed().as_millis() as u64;
        if let Some(stats) = &context.stats {
            let delta = Delta {
                videos: if request.audio { 0 } else { saved },
                audio: if request.audio { saved } else { 0 },
                failed,
                skipped,
                cancelled: u64::from(status == "cancelled"),
                elapsed_ms,
            };
            if let Err(error) = stats.record(delta) {
                context.stderr.write(&format!("veo: Could not save statistics: {}\n", readable_error(&error)));
            }
        }
        // History is written per finished item, so it never reports a running attempt.
        if let Some(history) = &context.history {
            let job_path = matches!(status.as_str(), "failed" | "cancelled").then(|| paths::display(&file));
            let item = HistoryItem {
                url: &request.url,
                title: title.as_deref(),
                status: &status,
                run_id: context.run_id.as_deref(),
                job: job_path.as_deref(),
                audio: request.audio,
                quality: &request.quality,
                format: request.format.as_deref(),
                files: &item_files,
                error: error_text.as_deref(),
                elapsed_ms,
            };
            if let Err(error) = history.record(&item) {
                context.stderr.write(&format!("veo: Could not save download history: {}\n", readable_error(&error)));
            }
        }
        persist()?;
        let mut totals = totals.lock().unwrap();
        totals.0 += saved;
        totals.1 += skipped;
        totals.2 += failed;
        Ok(())
    });
    if let Some(root) = &private_root {
        let _ = fs::remove_dir_all(root);
    }
    if let Err(error) = pool {
        if !context.cancel.is_cancelled() {
            return Err(error);
        }
    }
    let job = job.into_inner().unwrap();
    let (saved, skipped, failed) = *totals.lock().unwrap();
    let items = job["items"].as_array().cloned().unwrap_or_default();
    if let Some(id) = &context.run_id {
        let fingerprints = fingerprints.into_inner().unwrap();
        let outputs: Vec<String> = requests.iter().map(|request| request.output.clone()).collect();
        let files: Vec<Vec<String>> =
            items.iter().map(|item| item["files"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()).collect();
        let views: Vec<JobItemView> = items
            .iter()
            .enumerate()
            .map(|(index, item)| JobItemView {
                url: item["url"].as_str().unwrap_or(""),
                status: item["status"].as_str().unwrap_or("pending"),
                error: item["error"].as_str(),
                output: outputs.get(index).cloned().unwrap_or_else(|| options.output.clone()),
                files: &files[index],
                fingerprints: &fingerprints[index],
            })
            .collect();
        if let Err(error) = run_archive::archive_run(id, job["startedAt"].as_str(), &views, &context.archive_root) {
            context.stderr.write(&format!("veo: Could not archive run {id}: {}\n", readable_error(&error)));
        }
        context.stderr.write(&format!("Run ID: {id}\n"));
    }
    let unfinished = items.iter().any(|item| !matches!(item["status"].as_str(), Some("saved" | "skipped")));
    let download_count: usize = items.iter().map(|item| item["entries"].as_array().map_or(0, Vec::len).max(1)).sum();
    let cancelled = context.cancel.is_cancelled();
    let paint = color && !options.json;
    if download_count > 1 {
        let summary = format!("Summary: {saved} saved, {skipped} skipped, {failed} failed{}.", if cancelled { ", cancelled" } else { "" });
        context.stderr.write(&format!("{}\n", style(context.stderr.tty, &summary, if unfinished { "error" } else { "success" }, paint)));
    }
    if options.timings {
        let mut totals: Map<String, Value> = Map::new();
        for item in &items {
            let sources = std::iter::once(&item["timings"]).chain(item["entryTimings"].as_array().into_iter().flatten().map(|entry| &entry["timings"]));
            for timings in sources.filter_map(Value::as_object) {
                for (phase, ms) in timings {
                    let sum = totals.get(phase).and_then(Value::as_f64).unwrap_or(0.0) + ms.as_f64().unwrap_or(0.0);
                    totals.insert(phase.clone(), crate::downloader::number(sum));
                }
            }
        }
        if !totals.is_empty() {
            context.stderr.write(&format!("{}\n", style(context.stderr.tty, &format_timings(&totals), "muted", paint)));
        }
    }
    if unfinished && !options.incognito {
        let target = context.run_id.clone().unwrap_or_else(|| format!("\"{}\"", file.display()));
        context.stderr.write(&format!("Retry failed/unfinished downloads: veo --retry-failed {target}\n"));
    }
    if unfinished {
        context.reporter.fail(cancelled);
    } else {
        context.reporter.complete();
    }
    Ok(if cancelled {
        130
    } else if unfinished {
        1
    } else {
        0
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::NullReporter;
    use crate::terminal::Std;

    fn options(root: &Path, urls: &[&str]) -> Options {
        Options {
            quality: "best".into(),
            output: paths::display(root.join("out")),
            concurrent_downloads: 2,
            playlist_concurrency: 2,
            concurrent_fragments: 8,
            json: true,
            timings: false,
            urls: urls.iter().map(|url| url.to_string()).collect(),
            url: urls[0].into(),
            ..Options::default()
        }
    }

    #[test]
    fn records_failures_for_retry() {
        let root = crate::fsutil::temp_dir(&std::env::temp_dir(), "veo-jobs-").unwrap();
        let download = |request: &Options,
                        _: Arc<dyn Reporter>,
                        _: &Adaptive,
                        _: &(dyn Fn(&EntryOutcome) -> Result<()> + Sync),
                        _: Option<&Path>|
         -> Result<DownloadResult> {
            if request.url.contains("bad") {
                Err(Error::msg("ERROR: HTTP Error 404: Not Found"))
            } else {
                Ok(DownloadResult {
                    url: request.url.clone(),
                    title: Some("ok".into()),
                    files: vec![],
                    status: "saved".into(),
                    saved: 1,
                    ..DownloadResult::default()
                })
            }
        };
        let job_file = root.join("jobs").join(format!("{}-{}.json", state::now_ms(), state::uuid()));
        let options = options(&root, &["https://example.com/good", "https://example.com/bad"]);
        let code = run_job(
            &options,
            JobContext {
                download: &download,
                reporter: Arc::new(NullReporter),
                stdout: Out::new(Std::Out, true),
                stderr: Out::new(Std::Err, true),
                cancel: &Cancel::new(),
                open_file: &|_| Ok(()),
                items: None,
                job_file: Some(job_file.clone()),
                run_id: None,
                stats: None,
                history: Some(HistoryRecorder::new(&root)),
                archive_root: root.clone(),
            },
        )
        .unwrap();
        assert_eq!(code, 1);
        let retry = retry_options(&paths::display(&job_file), &root).unwrap();
        assert_eq!(retry.len(), 1);
        assert_eq!(retry[0]["url"], "https://example.com/bad");
        assert_eq!(retry[0]["resume"], true);
        assert_eq!(latest_failed_job(&root).unwrap(), job_file);
        let history = crate::history::read_history(&root, 5, false).unwrap();
        assert_eq!(history.len(), 2);
        let _ = fs::remove_dir_all(root);
    }
}
