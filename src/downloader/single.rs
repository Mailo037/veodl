//! Downloading one video: staging, backend run, compatibility, transfer.

use std::fs;
use std::path::Path;

use serde_json::{json, Value};

use crate::cancel::Cancel;
use crate::compatibility::ensure_compatibility;
use crate::disk_space::{estimate_media_bytes, Reservation};
use crate::download_cache::{self, TRANSFER_RETENTION_MS};
use crate::error::{optional, Error, Result};
use crate::execution::{adaptive_run, PhaseTimer};
use crate::metadata::{first, Format, Metadata};
use crate::naming::{media_destination, prepare_destination};
use crate::options::Options;
use crate::paths;
use crate::progress::{Processing, Progress};
use crate::run_archive::is_media_file;
use crate::state;
use crate::utils::save_unique;

use super::*;

pub(super) fn verify_files(files: &[String], options: &Options, deps: &Deps) -> Result<()> {
    let media: Vec<&String> = files.iter().filter(|file| is_media_file(file)).collect();
    if media.is_empty() {
        return Err(Error::msg("Verification failed: no saved media file was found."));
    }
    for file in media {
        (deps.verifier)(Path::new(file), options.audio, deps.cancel)?;
    }
    Ok(())
}

/// Download one URL. Returns every saved file in backend order, so playlist
/// entries and subtitle sidecars are reported.
pub fn download(options: &Options, deps: &Deps) -> Result<DownloadResult> {
    download_with(options, deps, None)
}

/// Like `download`, reusing metadata already read for the same request (saves
/// one full backend start, typically one to three seconds per URL).
pub fn download_with(options: &Options, deps: &Deps, prefetched: Option<Metadata>) -> Result<DownloadResult> {
    let timer = PhaseTimer::default();
    let cancel = deps.cancel;
    let reporter = deps.reporter.as_ref();
    let local_root = paths::resolve(&deps.local_root);
    if !deps.skip_cache_cleanup {
        download_cache::cleanup(&local_root, state::now_ms())?;
    }
    let request_key = local_request_key(options);
    let staging_path = local_root.join(format!("{PARTIAL_PREFIX}{request_key}"));
    if let Some(existing) = optional(fs::symlink_metadata(&staging_path))? {
        if !existing.is_dir() || existing.file_type().is_symlink() {
            return Err(Error::msg(format!("The partial download path is not a plain directory: {}", staging_path.display())));
        }
    }
    let cached: Option<Value> = if options.incognito { None } else { state::read_json(&staging_path.join("job.json"))? };
    let ready = read_ready_transfer(options, &local_root, cancel, cached.as_ref().or(Some(&Value::Null)).filter(|value| !value.is_null()), state::now_ms())?;
    let ready_manifest: Option<Manifest> = ready.as_ref().and_then(|value| serde_json::from_value(value.clone()).ok());
    let backend = if ready_manifest.is_some() { None } else { Some((deps.backend)(cancel, reporter)?) };
    timer.switch("metadata");
    let prefetched = prefetched.filter(|_| options.entry_index.is_none());
    let metadata = match (&ready_manifest, prefetched) {
        (Some(manifest), _) => manifest.metadata.clone(),
        (None, Some(metadata)) => announce(reporter, metadata),
        (None, None) => adaptive_run(options.adaptive_concurrency, deps.adaptive, cancel, reporter, Some(&timer), deps.wait, |_| {
            fetch_metadata(options, backend.as_ref().expect("backend"), deps.runner, cancel, reporter)
        })?,
    };
    if metadata.entries.is_some() && options.entry_index.is_none() {
        return download_collection(options, &metadata, deps, backend.as_ref().expect("backend for playlists"));
    }
    let destination = media_destination(options, &metadata, &staged_title("media.mp4", &metadata, options.rename.as_deref()))?;
    let directory = destination.directory.clone();
    prepare_destination(Path::new(&options.output), &directory, false)?;
    reporter.name(&destination.title);
    let quality = select_quality(&options.quality, options.audio, options.closest_quality, metadata.formats(), false);
    if let Some(error) = &quality.error {
        return Err(Error::msg(error.clone()));
    }
    if let Some(label) = &quality.label {
        reporter.status(label);
    }
    let timing_result = |timer: &PhaseTimer| if options.timings { Some(timer.result()) } else { None };

    let key = partial_key(&metadata, &options.url, options);
    let history_file = directory.join(".veo-history").join(format!("{key}.json"));
    let history: Option<HistoryRecord> = if !options.incognito && (options.skip_existing || (options.resume && options.entry_index.is_some())) {
        state::read_json(&history_file)?
    } else {
        None
    };
    if !options.incognito && prune_history_record(&history_file, &directory, history.as_ref()) {
        let files = history.as_ref().map(record_files).unwrap_or_default();
        if options.verify {
            if let Err(mut error) = verify_files(&files, options, deps) {
                error.files = files;
                error.verification_failed = true;
                return Err(error);
            }
        }
        reporter.status("Already downloaded; skipped.");
        return Ok(DownloadResult {
            url: options.url.clone(),
            title: metadata.title.clone(),
            files: Vec::new(),
            status: "skipped".into(),
            saved: 0,
            skipped: 1,
            verified: options.verify,
            timings: timing_result(&timer),
            ..DownloadResult::default()
        });
    }
    let staging = prepare_staging(&staging_path)?;
    let lock_path = staging.join(".lock");
    let lock = match fs::OpenOptions::new().write(true).create_new(true).open(&lock_path) {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(Error::msg(format!(
                "This download is already active, or a force-killed process left {}. Remove that lock only after checking that no other veo process uses it.",
                lock_path.display()
            )));
        }
        Err(error) => return Err(error.into()),
    };
    let manifest_file = staging.join("job.json");
    let cached_unconfirmed = cached.as_ref().is_some_and(|cached| {
        cached.get("ready").and_then(Value::as_array).is_some_and(|ready| !ready.is_empty())
            || cached.get("unconfirmedOutput").is_some_and(|value| value.as_bool() == Some(true))
    });
    let mut unconfirmed_output = ready_manifest.is_none() && cached_unconfirmed;
    let mut manifest: Option<Manifest> = None;
    let mut reservation: Option<Reservation> = None;
    let staging_display = paths::display(&staging);

    let result = (|| -> Result<DownloadResult> {
        let default_manifest = || Manifest {
            version: 1.into(),
            request_key: Value::String(request_key.clone()),
            metadata: Metadata {
                id: metadata.id.clone(),
                title: metadata.title.clone(),
                channel: metadata.channel.clone(),
                uploader: metadata.uploader.clone(),
                upload_date: metadata.upload_date.clone(),
                playlist_title: options.playlist_title.clone().or(metadata.playlist_title.clone()),
                extractor: metadata.extractor.clone(),
                extractor_key: metadata.extractor_key.clone(),
                formats: metadata.formats.as_ref().map(|formats| {
                    formats
                        .iter()
                        .map(|format| Format { height: format.height, vcodec: format.vcodec.clone(), has_drm: format.has_drm.clone(), ..Format::default() })
                        .collect()
                }),
                ..Metadata::default()
            },
            key: Value::String(key.clone()),
            source: Value::String(source_key(&metadata, &options.url)),
            settings: download_settings(options),
            ready: Vec::new(),
            files: Vec::new(),
            backend_succeeded: None,
            compatibility_checked: None,
            unconfirmed_output: None,
            expires_at: None,
        };
        let raw: Option<Value> = state::read_json(&manifest_file)?;
        let loaded = match raw {
            None => default_manifest(),
            Some(raw) => {
                if raw.get("key").and_then(Value::as_str) != Some(key.as_str()) || raw.get("version").and_then(Value::as_i64) != Some(1) {
                    return Err(Error::msg("Partial download belongs to different settings."));
                }
                if !manifest_shape(&raw) {
                    return Err(Error::msg("Invalid partial download manifest."));
                }
                serde_json::from_value(raw).map_err(|_| Error::msg("Invalid partial download manifest."))?
            }
        };
        let current = manifest.insert(loaded);
        if current.ready.iter().any(|file| paths::dirname(file) != staging || !is_staged_media(&paths::basename(file)))
            || current.files.iter().any(|item| paths::dirname(&item.source) != staging || paths::dirname(&item.destination) != directory)
        {
            return Err(Error::msg("Invalid partial download manifest."));
        }
        if options.check_space {
            let bytes = if ready_manifest.is_some() {
                let mut sum = 0.0;
                for file in &current.ready {
                    sum += fs::metadata(file)?.len() as f64;
                }
                Some(sum)
            } else {
                estimate_media_bytes(&metadata, options)
            };
            reservation = Some((deps.disk_checker)(&local_root, &directory, bytes, ready_manifest.is_some(), reporter)?);
        }
        current.expires_at = None;
        state::write_json(&manifest_file, current)?;
        // after_move can still be emitted after a media stream failed; a
        // successful process exit must confirm candidates before reuse.
        let mut staged: Vec<String> = if ready_manifest.is_some() { unique(&current.ready) } else { Vec::new() };
        if staged.is_empty() {
            let unconfirmed = current.backend_succeeded == Some(false) || !current.ready.is_empty();
            current.ready.clear();
            current.backend_succeeded = Some(false);
            state::write_json(&manifest_file, current)?;
            reporter.status(if options.audio { "Downloading audio…" } else { "Downloading…" });
            timer.switch("download");
            let backend = backend.as_ref().expect("backend for downloads");
            let template = staging_template(&staging, false);
            staged = adaptive_run(options.adaptive_concurrency, deps.adaptive, cancel, reporter, Some(&timer), deps.wait, |attempt| {
                let mut candidates: Vec<String> = Vec::new();
                let attempt_options = Options {
                    resume: options.resume || attempt > 0,
                    concurrent_fragments: deps.adaptive.reduced(options.concurrent_fragments),
                    ..options.clone()
                };
                let mut args = backend_args(&attempt_options, backend);
                args.extend(media_args(&attempt_options, &quality));
                // A failed process may have left a final filename: --no-overwrites
                // would otherwise silently accept that unconfirmed file on retry.
                if unconfirmed || attempt > 0 {
                    args.push("--force-overwrites".into());
                }
                args.extend(["-o".into(), template.clone(), "--".into(), options.media_url.clone().unwrap_or_else(|| options.url.clone())]);
                let mut on_line = |line: &str| -> Result<()> {
                    if let Some(rest) = line.strip_prefix("veo-progress:") {
                        if timer.phase() != "download" {
                            timer.switch("download");
                        }
                        let data: Value = serde_json::from_str(rest)?;
                        let video = data.get("video").and_then(Value::as_str);
                        let audio = data.get("audio").and_then(Value::as_str);
                        let mut progress: Progress = match data.get("progress") {
                            Some(progress) if progress.is_object() => serde_json::from_value(progress.clone()).unwrap_or_default(),
                            _ => serde_json::from_value(data.clone()).unwrap_or_default(),
                        };
                        if data.get("progress").is_some_and(Value::is_object) {
                            progress.stream = Some(
                                if video == Some("none") {
                                    "Audio"
                                } else if audio == Some("none") {
                                    "Video"
                                } else {
                                    "Media"
                                }
                                .into(),
                            );
                        }
                        reporter.progress(&progress);
                    }
                    if let Some(rest) = line.strip_prefix("veo-postprocess:") {
                        if timer.phase() != "processing" {
                            timer.switch("processing");
                        }
                        let data: Processing = serde_json::from_str(rest).unwrap_or_default();
                        reporter.processing(&data);
                    }
                    if let Some(rest) = line.strip_prefix("veo-file:") {
                        let file: String = serde_json::from_str(rest)?;
                        if paths::dirname(paths::resolve(&file)) != staging || !is_staged_media(&paths::basename(&file)) {
                            return Err(Error::msg("The backend returned an invalid saved file path."));
                        }
                        candidates.push(file);
                        unconfirmed_output = true;
                    }
                    Ok(())
                };
                (deps.runner)(&backend.yt_dlp, &args, cancel, Some(&mut on_line))?;
                Ok(unique(&candidates))
            })?;
            current.ready = staged.clone();
            current.backend_succeeded = Some(true);
            current.unconfirmed_output = None;
            unconfirmed_output = false;
            state::write_json(&manifest_file, current)?;
        }
        // Cancellation wins over any follow-up error.
        cancel.check()?;
        if staged.is_empty() {
            return Err(Error::msg("The backend finished without producing a file."));
        }
        if !options.audio && current.compatibility_checked != Some(true) && (ready_manifest.is_none() || options.compatible) {
            timer.switch("processing");
            let tools = match &backend {
                Some(backend) => backend.clone(),
                None => (deps.backend)(cancel, reporter)?,
            };
            for file in staged.iter_mut() {
                *file = paths::display(ensure_compatibility(Path::new(file), &tools, deps.compatibility_runner, cancel, options.compatible, reporter)?);
            }
            current.ready = staged.clone();
            current.compatibility_checked = Some(true);
            state::write_json(&manifest_file, current)?;
        }
        let mut files: Vec<String> = Vec::new();
        timer.switch("saving");
        reporter.status(if ready_manifest.is_some() { "Retrying transfer from local cache…" } else { "Saving local download to destination…" });
        prepare_destination(Path::new(&options.output), &directory, true)?;
        for staged_file in &staged {
            let resolved = paths::resolve(staged_file);
            // The backend must only ever hand back a file inside our staging directory.
            if paths::dirname(&resolved) != staging || !fs::symlink_metadata(&resolved)?.is_file() {
                return Err(Error::msg("The backend returned an invalid saved file path."));
            }
            let resolved_text = paths::display(&resolved);
            let record =
                current.files.iter().find(|item| item.source == resolved_text && fs::metadata(&item.destination).is_ok_and(|info| info.is_file())).cloned();
            let saved = match &record {
                Some(record) => record.destination.clone(),
                None => {
                    let saved = paths::display(save_unique(&resolved, &directory, &destination.title, cancel, true)?);
                    current.files.retain(|item| item.source != resolved_text);
                    current.files.push(FileRecord { source: resolved_text.clone(), destination: saved.clone() });
                    state::write_json(&manifest_file, current)?;
                    saved
                }
            };
            files.push(saved.clone());
            files.extend(save_sidecars(&staging, &resolved, Path::new(&saved), cancel, current, &manifest_file)?);
        }
        if options.verify {
            reporter.status("Verifying saved media…");
            if let Err(mut error) = verify_files(&files, options, deps) {
                error.files = files.clone();
                error.verification_failed = true;
                return Err(error);
            }
            reporter.status("Media verification passed.");
        }
        if !options.incognito {
            state::write_json(
                &history_file,
                &json!({ "version": 1, "source": source_key(&metadata, &options.url), "settings": download_settings(options), "files": files }),
            )?;
        }
        Ok(DownloadResult {
            url: options.url.clone(),
            title: Some(first(&[&metadata.title, &metadata.id]).unwrap_or("video").to_string()),
            files,
            status: "saved".into(),
            saved: 1,
            skipped: 0,
            verified: options.verify,
            timings: timing_result(&timer),
            ..DownloadResult::default()
        })
    })();

    let mut keep_partial = false;
    if let Err(error) = &result {
        if !cancel.is_cancelled() {
            reporter.fail_step();
        }
        // A kept staging directory is the whole point of --resume, and
        // cancellation is the most common reason to want one.
        let ready_count = manifest.as_ref().map_or(0, |manifest| manifest.ready.len());
        keep_partial = !options.incognito && !error.verification_failed && (options.resume || ready_count > 0 || unconfirmed_output);
        if let Some(manifest) = manifest.as_mut() {
            if unconfirmed_output {
                manifest.unconfirmed_output = Some(true);
            }
            if keep_partial && (ready_count > 0 || (unconfirmed_output && !options.resume)) {
                manifest.expires_at = Some((state::now_ms() + TRANSFER_RETENTION_MS) as f64);
                let _ = state::write_json(&manifest_file, manifest);
            }
        }
    }
    drop(reservation);
    drop(lock);
    let _ = fs::remove_file(&lock_path);
    reporter.finish();
    if keep_partial {
        let expires = manifest.as_ref().is_some_and(|manifest| manifest.expires_at.is_some());
        let ready = manifest.as_ref().is_some_and(|manifest| !manifest.ready.is_empty());
        reporter.status(&if expires && !ready {
            format!("Unconfirmed download kept locally in {staging_display} for 15 minutes. Retry will rerun the backend; incomplete media may need to be downloaded again.")
        } else if expires {
            format!("Completed download kept locally in {staging_display}. Retry within 15 minutes to transfer without downloading again. Expired files are cleaned on the next veo run.")
        } else {
            format!("Partial download kept locally in {staging_display}. Re-run with --resume to continue it.")
        });
    } else {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

/// Move subtitle and thumbnail files beside the saved media, keeping the media
/// name as prefix (Title.mp4 -> Title.en.vtt).
pub(super) fn save_sidecars(
    staging: &Path,
    staged_media: &Path,
    saved_media: &Path,
    cancel: &Cancel,
    manifest: &mut Manifest,
    manifest_file: &Path,
) -> Result<Vec<String>> {
    let mut written = Vec::new();
    let stem = paths::basename(staged_media);
    let stem = stem.strip_suffix(&paths::extname(staged_media)).unwrap_or(&stem).to_string();
    let base = paths::basename(saved_media);
    let base = base.strip_suffix(&paths::extname(saved_media)).unwrap_or(&base).to_string();
    let mut names: Vec<String> = fs::read_dir(staging)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    for name in names {
        if !is_sidecar(&name) || !name.starts_with(&format!("{stem}.")) {
            continue;
        }
        let without_extension = name.strip_suffix(&paths::extname(&name)).unwrap_or(&name);
        let suffix = &without_extension[stem.len()..];
        let source = paths::display(staging.join(&name));
        if let Some(record) = manifest.files.iter().find(|item| item.source == source) {
            if fs::metadata(&record.destination).is_ok_and(|info| info.is_file()) {
                written.push(record.destination.clone());
                continue;
            }
        }
        let destination = paths::display(save_unique(Path::new(&source), &paths::dirname(saved_media), &format!("{base}{suffix}"), cancel, true)?);
        manifest.files.retain(|item| item.source != source);
        manifest.files.push(FileRecord { source: source.clone(), destination: destination.clone() });
        state::write_json(manifest_file, manifest)?;
        written.push(destination);
    }
    Ok(written)
}
