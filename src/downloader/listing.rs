//! Metadata reads and the actions that only inspect: listings and dry runs.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Map, Value};

use crate::backend::{self, Backend};
use crate::cancel::Cancel;
use crate::error::{Error, Result};
use crate::metadata::{first, Format, Metadata};
use crate::naming::{media_destination, prepare_destination};
use crate::options::Options;
use crate::paths;
use crate::playlist::{selected_entries, size_estimate, Estimate};
use crate::progress::Reporter;
use crate::utils::{available_heights, sanitize_title};

use super::*;

/// Raw extractor data can contain signed URLs and headers. Keep it in an
/// exclusive private file only while the child process needs it.
pub(super) struct ExtractionFile(PathBuf);

impl ExtractionFile {
    pub fn write(staging: &Path, metadata: &Metadata) -> Result<Option<Self>> {
        use std::io::Write;
        let Some(extraction) = &metadata.extraction else { return Ok(None) };
        let file = Self(staging.join(format!("extractor-{}.json", crate::state::uuid())));
        let mut handle = crate::state::create_private(&file.0)?;
        serde_json::to_writer(&mut handle, extraction.as_ref())?;
        handle.flush()?;
        Ok(Some(file))
    }

    pub fn path(&self) -> String {
        paths::display(&self.0)
    }
}

impl Drop for ExtractionFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub(super) fn stale_extraction(error: &Error) -> bool {
    re!(r"(?i)HTTP(?: Error)?\s*:?\s*(?:401|403|410)|expired|requested format.*not available|no video formats").is_match(&error.message)
}

/// Read video metadata once, with the same vetted arguments as a download.
pub fn fetch_metadata(options: &Options, backend: &Backend, runner: &Runner, cancel: &Cancel, reporter: &dyn Reporter) -> Result<Metadata> {
    reporter.status("Reading video…");
    let mut args = backend_args(options, backend);
    args.extend(s(&["--dump-single-json", "--skip-download"]));
    if options.playlist && options.entry_index.is_none() {
        args.push("--flat-playlist".into());
    }
    args.push("--".into());
    args.push(options.entry_url.clone().or_else(|| options.media_url.clone()).unwrap_or_else(|| options.url.clone()));
    let result = (|| -> Result<Metadata> {
        let raw = runner(&backend.yt_dlp, &args, cancel, None)?;
        let mut metadata: Metadata = serde_json::from_str(&raw).map_err(|error| Error::msg(format!("The backend returned invalid metadata: {error}")))?;
        if options.entry_index.is_some() && metadata.entries.is_some() {
            metadata = metadata
                .entries
                .take()
                .unwrap_or_default()
                .into_iter()
                .flatten()
                .next()
                .ok_or_else(|| Error::msg("The selected playlist entry is unavailable."))?;
        }
        // Keep flat playlist dumps lean. Single-video results retain all backend
        // fields (headers, fragment tables, format IDs) for --load-info-json.
        if !metadata.is_collection() {
            let mut extraction: Value = serde_json::from_str(&raw)?;
            if options.entry_index.is_some() && extraction.get("entries").is_some() {
                extraction = extraction
                    .get_mut("entries")
                    .and_then(Value::as_array_mut)
                    .and_then(|entries| entries.iter_mut().find(|entry| entry.is_object()))
                    .map(Value::take)
                    .unwrap_or(Value::Null);
            }
            if extraction.is_object() {
                metadata.extraction = Some(std::sync::Arc::new(extraction));
            }
        }
        if !options.playlist && metadata.is_collection() {
            return Err(Error::msg("This URL is a collection. Add --playlist to download every entry."));
        }
        if metadata.live() {
            return Err(Error::msg("Live streams are not supported. Please use a finished video."));
        }
        if metadata.drm() {
            return Err(Error::msg("This content is DRM-protected."));
        }
        Ok(metadata)
    })();
    match &result {
        Ok(_) => reporter.finish_status("Reading video…", "done"),
        Err(error) if !cancel.is_cancelled() && !error.is_cancelled() => reporter.finish_status("Reading video…", "failed"),
        Err(_) => {}
    }
    result
}

/// Show the metadata step for metadata that was already read for this request.
pub(super) fn announce(reporter: &dyn Reporter, metadata: Metadata) -> Metadata {
    reporter.status("Reading video…");
    reporter.finish_status("Reading video…", "done");
    metadata
}

/// Resolves the backend once and shares it (e.g. between discovery and download).
pub type BackendResolver<'a> = dyn Fn(&Cancel, &dyn Reporter) -> Result<Backend> + Sync + 'a;

pub fn default_backend(cancel: &Cancel, reporter: &dyn Reporter) -> Result<Backend> {
    backend::resolve_backend(cancel, false, &|message| reporter.status(message), &|message, outcome| reporter.finish_status(message, outcome))
}

/// The extension the finished file will most likely have, for previews.
pub fn predicted_extension(options: &Options) -> String {
    options.format.clone().filter(|format| !format.is_empty()).unwrap_or_else(|| if options.audio { "mp3".into() } else { "mp4".into() })
}

/// First free destination name, without creating anything (preview only).
pub fn preview_path(directory: &Path, title: &str, extension: &str, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    let name = sanitize_title(title);
    for number in 0u64.. {
        let suffix = if number > 0 { format!(" ({number})") } else { String::new() };
        let candidate = directory.join(format!("{name}{suffix}.{extension}"));
        if !exists(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

pub struct Tools<'a> {
    pub cancel: &'a Cancel,
    pub reporter: &'a dyn Reporter,
    pub backend: &'a BackendResolver<'a>,
    pub runner: &'a Runner<'a>,
}

pub(super) fn single(options: &Options) -> Options {
    Options { playlist: false, ..options.clone() }
}

/// Summarize source resolutions without downloading media.
pub fn list_qualities(options: &Options, tools: &Tools, prefetched: Option<Metadata>) -> Result<Value> {
    let metadata = match prefetched {
        Some(metadata) => announce(tools.reporter, metadata),
        None => fetch_metadata(&single(options), &(tools.backend)(tools.cancel, tools.reporter)?, tools.runner, tools.cancel, tools.reporter)?,
    };
    let formats: Vec<Format> = match &metadata.formats {
        Some(formats) => formats.clone(),
        None => vec![metadata.as_format()],
    };
    let formats: Vec<Format> = formats
        .into_iter()
        .filter(|format| format.is_video() && !format.drm() && format.ext.as_deref() != Some("mhtml") && format.protocol.as_deref() != Some("mhtml"))
        .collect();
    if formats.is_empty() {
        return Err(Error::msg("No downloadable video formats found."));
    }
    let mut heights = available_heights(&formats);
    heights.sort_unstable_by(|a, b| b.cmp(a));
    Ok(
        json!({ "url": options.url, "status": "qualities", "title": text_or_null(&metadata.title), "qualities": heights.iter().map(|height| format!("{height}p")).collect::<Vec<_>>() }),
    )
}

/// List subtitle languages and source formats without exposing signed URLs.
pub fn list_subtitles(options: &Options, tools: &Tools, prefetched: Option<Metadata>) -> Result<Value> {
    let metadata = match prefetched {
        Some(metadata) => announce(tools.reporter, metadata),
        None => fetch_metadata(&single(options), &(tools.backend)(tools.cancel, tools.reporter)?, tools.runner, tools.cancel, tools.reporter)?,
    };
    let summarize = |source: &Option<Map<String, Value>>| -> Vec<Value> {
        let mut items: Vec<(String, Value)> = source
            .iter()
            .flatten()
            .map(|(language, tracks)| {
                let list = tracks.as_array();
                let name = list
                    .and_then(|tracks| tracks.iter().find_map(|track| track.get("name").and_then(Value::as_str)))
                    .map_or(Value::Null, |name| Value::String(name.into()));
                let mut formats: Vec<String> = list
                    .map(|tracks| {
                        tracks
                            .iter()
                            .filter_map(|track| track.get("ext").and_then(Value::as_str))
                            .filter(|ext| re!(r"(?i)^[a-z0-9]+$").is_match(ext))
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                formats.sort();
                formats.dedup();
                (language.clone(), json!({ "language": language, "name": name, "formats": formats }))
            })
            .collect();
        items.sort_by(|a, b| crate::utils::locale_cmp(&a.0, &b.0));
        items.into_iter().map(|(_, value)| value).collect()
    };
    Ok(json!({
        "url": options.url, "status": "subtitles", "title": text_or_null(&metadata.title),
        "manual": summarize(&metadata.subtitles), "automatic": summarize(&metadata.automatic_captions),
    }))
}

/// `veo --list-formats`: the backend's own format table.
pub fn list_formats(options: &Options, tools: &Tools) -> Result<String> {
    let backend = (tools.backend)(tools.cancel, tools.reporter)?;
    tools.reporter.status("Reading available formats…");
    let mut args = backend_args(options, &backend);
    args.extend(["-F".to_string(), "--".into(), options.media_url.clone().unwrap_or_else(|| options.url.clone())]);
    (tools.runner)(&backend.yt_dlp, &args, tools.cancel, None)
}

/// A JSON number without a trailing `.0` for whole values, like JavaScript.
pub fn number(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 9e15 {
        Value::from(value as i64)
    } else {
        Value::from(value)
    }
}

pub(super) fn estimate_json(estimate: &Estimate, map: &mut Map<String, Value>) {
    map.insert("estimatedBytes".into(), number(estimate.estimated_bytes));
    map.insert("knownSizes".into(), estimate.known_sizes.into());
    map.insert("totalEntries".into(), estimate.total_entries.into());
}

/// `veo --dry-run`: inspect and report what would happen without writing.
pub fn plan_download(options: &Options, tools: &Tools, prefetched: Option<Metadata>) -> Result<(Map<String, Value>, Estimate)> {
    let directory = paths::resolve(&options.output);
    let metadata = match prefetched {
        Some(metadata) => announce(tools.reporter, metadata),
        None => fetch_metadata(options, &(tools.backend)(tools.cancel, tools.reporter)?, tools.runner, tools.cancel, tools.reporter)?,
    };
    let quality = select_quality(&options.quality, options.audio, options.closest_quality, metadata.formats(), options.playlist);
    if let Some(error) = quality.error {
        return Err(Error::msg(error));
    }
    let selected = if metadata.entries.is_some() {
        selected_entries(&metadata, options.playlist_items.as_deref())?
    } else {
        vec![crate::playlist::Selected { entry: Some(&metadata), index: 1 }]
    };
    let collection = options.playlist && metadata.has_entries();
    let titles: Vec<String> = if collection {
        selected
            .iter()
            .map(|item| {
                let entry = item.entry.cloned().unwrap_or_default();
                match options.rename.as_deref() {
                    Some(rename) if rename.contains('*') => staged_title("media.mp4", &entry, Some(rename)),
                    Some(rename) if !rename.is_empty() => format!("{rename} - {:03}", item.index),
                    _ => first(&[&entry.title])
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("{} - {}", first(&[&metadata.title]).unwrap_or("video"), item.index)),
                }
            })
            .collect()
    } else {
        vec![staged_title("media.mp4", &metadata, options.rename.as_deref())]
    };
    let extension = predicted_extension(options);
    let reserved: Mutex<HashSet<PathBuf>> = Mutex::new(HashSet::new());
    let exists = |candidate: &Path| reserved.lock().unwrap().contains(candidate) || fs::metadata(candidate).is_ok();
    let mut planned = Vec::new();
    for (offset, title) in titles.iter().enumerate() {
        let item = &selected[offset];
        let entry = item.entry.cloned().unwrap_or_default();
        let request = Options {
            entry_index: if metadata.entries.is_some() { Some(item.index) } else { options.entry_index },
            playlist_title: if metadata.entries.is_some() { metadata.title.clone() } else { options.playlist_title.clone() },
            ..options.clone()
        };
        let destination = media_destination(&request, &entry, title)?;
        prepare_destination(&directory, &destination.directory, false)?;
        let target = preview_path(&destination.directory, &destination.title, &extension, &exists);
        reserved.lock().unwrap().insert(target.clone());
        planned.push(json!({ "title": title, "path": paths::display(&target) }));
    }
    let estimate = size_estimate(&selected);
    let mut plan = Map::new();
    plan.insert("url".into(), Value::String(options.url.clone()));
    plan.insert("playlist".into(), options.playlist.into());
    plan.insert("quality".into(), quality.label.map_or(Value::Null, Value::String));
    plan.insert("entries".into(), Value::Array(planned));
    estimate_json(&estimate, &mut plan);
    Ok((plan, estimate))
}
