//! Retry jobs: finding them by run ID and reading their pending items.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::error::{optional, Error, Result};
use crate::paths;
use crate::run_archive;
use crate::state;
use crate::utils::validate_url;

pub(super) const JOB_NAME: &str = r"^[0-9]{13}-[a-f0-9-]{36}\.json$";

pub(super) fn read_job(file: &Path) -> Result<Option<Value>> {
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

pub(super) fn job_names(directory: &Path) -> Result<Vec<String>> {
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
pub(super) fn retry_job_for_run(id: &str, root: &Path) -> Result<PathBuf> {
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
