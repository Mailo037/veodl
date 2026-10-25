//! Downloading the selected entries of a playlist in parallel.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};

use crate::backend::Backend;
use crate::cancel::Cancel;
use crate::error::{Error, Result};
use crate::metadata::{first, Metadata};
use crate::options::Options;
use crate::playlist::{describe_estimate, selected_entries, size_estimate};
use crate::progress::Reporter;

use super::*;

pub(super) fn download_collection(options: &Options, metadata: &Metadata, deps: &Deps, backend: &Backend) -> Result<DownloadResult> {
    let reporter = deps.reporter.clone();
    let entries = selected_entries(metadata, options.playlist_items.as_deref())?;
    reporter.status(&describe_estimate(&size_estimate(&entries)));
    let concurrency = options.playlist_concurrency.max(1);
    if !(1..=4).contains(&options.playlist_concurrency) {
        return Err(Error::msg("Playlist concurrency must be between 1 and 4."));
    }
    let total = metadata.entries.as_ref().map_or(0, Vec::len);
    let shared_backend = |_: &Cancel, _: &dyn Reporter| Ok(backend.clone());
    let child_deps = Deps { backend: &shared_backend, ..deps.with_reporter(reporter.clone()) };
    let results: Vec<Mutex<Vec<String>>> = entries.iter().map(|_| Mutex::new(Vec::new())).collect();
    let failures: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());
    let entry_timings: Mutex<Vec<(u64, Map<String, Value>)>> = Mutex::new(Vec::new());
    let (saved, skipped) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let notify = Mutex::new(());
    let next = AtomicUsize::new(0);
    let fatal: Mutex<Option<Error>> = Mutex::new(None);
    let playlist_title = first(&[&metadata.title, &metadata.id]).map(str::to_string);
    std::thread::scope(|scope| {
        for _worker in 0..(concurrency as usize).min(entries.len()) {
            let (entries, results, failures, entry_timings, saved, skipped, notify, next, fatal, child_deps, reporter, playlist_title) =
                (&entries, &results, &failures, &entry_timings, &saved, &skipped, &notify, &next, &fatal, &child_deps, &reporter, &playlist_title);
            scope.spawn(move || {
                while next.load(Ordering::SeqCst) < entries.len() {
                    if let Err(error) = deps.cancel.check() {
                        fatal.lock().unwrap().get_or_insert(error);
                        return;
                    }
                    let offset = next.fetch_add(1, Ordering::SeqCst);
                    let Some(item) = entries.get(offset) else { return };
                    let index = item.index;
                    let title = item.entry.and_then(|entry| first(&[&entry.title]).map(str::to_string)).unwrap_or_else(|| format!("Entry {index}"));
                    let child: Arc<dyn Reporter> =
                        if concurrency > 1 { reporter.scoped(index as usize, total, &title).unwrap_or_else(|| reporter.clone()) } else { reporter.clone() };
                    if concurrency == 1 {
                        reporter.item(offset + 1, entries.len(), &title);
                    }
                    let rename = match options.rename.as_deref() {
                        Some(rename) if rename.contains('*') => Some(rename.to_string()),
                        Some(rename) if !rename.is_empty() => Some(format!("{rename} - {index:03}")),
                        _ => None,
                    };
                    let entry_url = item
                        .entry
                        .and_then(|entry| entry.webpage_url.as_ref().or(entry.url.as_ref()))
                        .filter(|url| crate::utils::validate_url(url).is_ok())
                        .cloned();
                    let item_options = Options { entry_index: Some(index), entry_url, playlist_title: playlist_title.clone(), rename, ..options.clone() };
                    let outcome = match download(&item_options, &child_deps.with_reporter(child)) {
                        Ok(result) => {
                            *results[offset].lock().unwrap() = result.files.clone();
                            if let Some(timings) = &result.timings {
                                entry_timings.lock().unwrap().push((index, timings.clone()));
                            }
                            if result.status == "skipped" {
                                skipped.fetch_add(1, Ordering::SeqCst)
                            } else {
                                saved.fetch_add(1, Ordering::SeqCst)
                            };
                            EntryOutcome { index, status: result.status.clone(), files: result.files, error: None, timings: result.timings }
                        }
                        Err(error) => {
                            if deps.cancel.is_cancelled() || error.is_cancelled() {
                                fatal.lock().unwrap().get_or_insert(error);
                                return;
                            }
                            failures.lock().unwrap().push((index, error.message.clone()));
                            *results[offset].lock().unwrap() = error.files.clone();
                            reporter.status(&format!("Entry {index} failed: {}", error.message));
                            EntryOutcome { index, status: "failed".into(), files: error.files.clone(), error: Some(error.message), timings: None }
                        }
                    };
                    if let Some(on_entry) = deps.on_entry {
                        let _guard = notify.lock().unwrap();
                        if let Err(error) = on_entry(&outcome) {
                            fatal.lock().unwrap().get_or_insert(error);
                            return;
                        }
                    }
                }
            });
        }
    });
    if let Some(error) = fatal.into_inner().unwrap() {
        return Err(error);
    }
    let files: Vec<String> = results.into_iter().flat_map(|files| files.into_inner().unwrap()).collect();
    let mut failures = failures.into_inner().unwrap();
    failures.sort_by_key(|(index, _)| *index);
    let mut timings = entry_timings.into_inner().unwrap();
    timings.sort_by_key(|(index, _)| *index);
    let (saved, skipped) = (saved.into_inner() as u64, skipped.into_inner() as u64);
    Ok(DownloadResult {
        url: options.url.clone(),
        title: Some(first(&[&metadata.title, &metadata.id]).unwrap_or("Playlist").to_string()),
        files,
        status: if !failures.is_empty() {
            "failed"
        } else if saved > 0 {
            "saved"
        } else {
            "skipped"
        }
        .into(),
        saved,
        skipped,
        verified: options.verify && failures.is_empty() && skipped == 0,
        failures: Some(failures.into_iter().map(|(index, error)| json!({ "index": index, "error": error })).collect()),
        timings: None,
        entry_timings: timings.into_iter().map(|(index, timings)| json!({ "index": index, "timings": timings })).collect(),
    })
}
