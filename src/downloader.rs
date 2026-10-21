use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::backend::{self, Backend};
use crate::cancel::Cancel;
use crate::compatibility::ensure_compatibility;
use crate::disk_space::{estimate_media_bytes, reserve_space, Reservation};
use crate::download_cache::{self, TRANSFER_RETENTION_MS};
use crate::error::{optional, Error, Result};
use crate::execution::{adaptive_run, Adaptive, PhaseTimer};
use crate::metadata::{first, Format, Metadata};
use crate::naming::{media_destination, prepare_destination};
use crate::options::Options;
use crate::paths;
use crate::playlist::{describe_estimate, selected_entries, size_estimate, Estimate};
use crate::process::{self, Command, Lines, Stream};
use crate::progress::{Processing, Progress, Reporter};
use crate::run_archive::is_media_file;
use crate::state::{self, digest};
use crate::utils::{available_heights, capped_height, closest_height, sanitize_title, save_unique};

const PARTIAL_PREFIX: &str = ".veo-part-";
// Only real media extensions may count as the downloaded file, so a thumbnail
// or subtitle the backend leaves behind is never mistaken for it.
const MEDIA_EXTENSIONS: [&str; 23] = [
    "mp4", "mkv", "webm", "mov", "avi", "m4v", "3gp", "ts", "flv", "mpg", "mpeg", "ogv", "mp3", "m4a", "aac", "opus", "flac", "wav", "ogg", "oga", "wma",
    "mka", "aiff",
];
// Formats the backend may leave beside the media file. Merge fragments such as
// media.f137.mp4 are never deliverables.
const SIDECAR_EXTENSIONS: [&str; 13] = ["vtt", "srt", "ass", "lrc", "ttml", "srv1", "srv2", "srv3", "json", "jpg", "jpeg", "png", "webp"];

pub fn is_staged_media(name: &str) -> bool {
    re!(r"^media(?:-([0-9]+))?\.([A-Za-z0-9]{1,8})$").captures(name).is_some_and(|captures| MEDIA_EXTENSIONS.contains(&captures[2].to_lowercase().as_str()))
}

fn staged_index(name: &str) -> Option<String> {
    if !is_staged_media(name) {
        return None;
    }
    re!(r"^media(?:-([0-9]+))?\.").captures(name).and_then(|captures| captures.get(1).map(|index| index.as_str().to_string()))
}

/// Runs a backend tool: `(executable, args, cancel, on_line)` → stdout.
pub type Runner<'a> = dyn Fn(&Path, &[String], &Cancel, Option<&mut dyn FnMut(&str) -> Result<()>>) -> Result<String> + Sync + 'a;

/// Run yt-dlp (or ffprobe). With `on_line`, stdout lines and veo progress lines
/// from stderr are streamed; otherwise stdout is returned (max 32 MiB).
pub fn run_backend(executable: &Path, args: &[String], cancel: &Cancel, mut on_line: Option<&mut dyn FnMut(&str) -> Result<()>>) -> Result<String> {
    let command = Command::new(executable).args(args.iter().cloned());
    let mut output: Vec<u8> = Vec::new();
    let mut errors = String::new();
    let (mut out_lines, mut err_lines) = (Lines::default(), Lines::default());
    let streaming = on_line.is_some();
    let code = process::run(&command, cancel, |stream, data| {
        match stream {
            Stream::Stdout => {
                if let Some(on_line) = on_line.as_mut() {
                    out_lines.push(data, |line| on_line(line))?;
                } else if output.len() + data.len() < 32 * 1024 * 1024 {
                    output.extend_from_slice(data);
                } else {
                    return Err(Error::msg("Video metadata exceeded the supported size."));
                }
            }
            Stream::Stderr => {
                errors.push_str(&String::from_utf8_lossy(data));
                if errors.len() > 16000 {
                    let cut = errors.len() - 16000;
                    let boundary = (cut..errors.len()).find(|index| errors.is_char_boundary(*index)).unwrap_or(errors.len());
                    errors.drain(..boundary);
                }
                if let Some(on_line) = on_line.as_mut() {
                    err_lines
                        .push(data, |line| if line.starts_with("veo-progress:") || line.starts_with("veo-postprocess:") { on_line(line) } else { Ok(()) })?;
                }
            }
        }
        Ok(())
    })?;
    if let Some(on_line) = on_line.as_mut() {
        out_lines.finish(|line| on_line(line))?;
    }
    if code != Some(0) {
        let diagnostics: Vec<&str> = errors.lines().filter(|line| !re!(r"^veo-(?:progress|postprocess):").is_match(line)).collect();
        let failure = diagnostics.iter().rev().find(|line| re!(r"(?i)^ERROR:").is_match(line));
        let message = match failure {
            Some(line) => line.to_string(),
            None if !diagnostics.join("\n").is_empty() => diagnostics.join("\n"),
            None => format!("Downloading backend exited with code {}.", code.map_or("null".into(), |code| code.to_string())),
        };
        return Err(Error::msg(message));
    }
    let _ = streaming;
    Ok(String::from_utf8_lossy(&output).into_owned())
}

pub fn format_selector(height: Option<u32>) -> String {
    let filter = height.map(|height| format!("[height={height}]")).unwrap_or_default();
    format!("bv{filter}+ba/b{filter}/bv{filter}")
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct QualityChoice {
    pub format: Option<String>,
    pub sort: Option<String>,
    pub label: Option<String>,
    pub error: Option<String>,
}

/// Turn a requested quality into backend arguments and a user-facing label.
/// Numeric qualities are an upper bound; `closest` picks the nearest height.
pub fn select_quality(quality: &str, audio: bool, closest: bool, formats: &[Format], playlist: bool) -> QualityChoice {
    if audio || quality == "best" {
        return QualityChoice::default();
    }
    let target = crate::utils::quality_target(quality);
    if playlist {
        // Entries differ in resolution, so cap them inside the backend instead.
        return QualityChoice {
            sort: Some(format!("res:{target}")),
            label: Some(if closest { format!("Quality: closest to {target}p per entry") } else { format!("Quality: up to {target}p per entry") }),
            ..QualityChoice::default()
        };
    }
    let heights = available_heights(formats);
    if heights.is_empty() {
        return QualityChoice { label: Some("Resolution unknown; using the best available stream.".into()), ..QualityChoice::default() };
    }
    if closest {
        let height = closest_height(formats, quality);
        let label = match height {
            Some(height) if height == target => format!("Quality: {height}p"),
            Some(height) => format!("Quality: {height}p (closest to {quality})"),
            None => "Quality: best".into(),
        };
        return QualityChoice { format: Some(format_selector(height)), label: Some(label), ..QualityChoice::default() };
    }
    let Some(height) = capped_height(formats, quality) else {
        let mut offered = heights.clone();
        offered.sort_unstable();
        let offered = offered.iter().map(|value| format!("{value}p")).collect::<Vec<_>>().join(", ");
        return QualityChoice {
            error: Some(format!("No stream at or below {target}p is available (offered: {offered}). Use a higher --quality or \"best\".")),
            ..QualityChoice::default()
        };
    };
    let label = if height == target { format!("Quality: {height}p") } else { format!("Quality: {height}p (highest at or below {quality})") };
    QualityChoice { format: Some(format_selector(Some(height))), label: Some(label), ..QualityChoice::default() }
}

fn source_key(metadata: &Metadata, url: &str) -> String {
    match (first(&[&metadata.id]), first(&[&metadata.extractor_key, &metadata.extractor])) {
        (Some(id), Some(extractor)) => format!("{extractor}:{id}"),
        _ => format!("{url}#{}", metadata.id.as_deref().unwrap_or("")),
    }
}

fn text_or_null(value: &Option<String>) -> Value {
    value.as_ref().map_or(Value::Null, |text| Value::String(text.clone()))
}

/// Settings that change the produced file; part of every staging key.
fn download_settings(options: &Options) -> Value {
    let mut settings = Map::new();
    settings.insert("quality".into(), Value::String(options.quality.clone()));
    settings.insert("audio".into(), options.audio.into());
    settings.insert("format".into(), text_or_null(&options.format));
    settings.insert("closestQuality".into(), options.closest_quality.into());
    settings.insert("subs".into(), options.subs.into());
    settings.insert("subLangs".into(), text_or_null(&options.sub_langs));
    settings.insert("embedSubs".into(), options.embed_subs.into());
    settings.insert("embedMetadata".into(), options.embed_metadata.into());
    settings.insert("embedThumbnail".into(), options.embed_thumbnail.into());
    settings.insert("sponsorblockRemove".into(), text_or_null(&options.sponsorblock_remove));
    settings.insert("section".into(), text_or_null(&options.section));
    if options.auto_subs {
        settings.insert("autoSubs".into(), true.into());
    }
    if let Some(format) = options.sub_format.as_ref().filter(|format| !format.is_empty()) {
        settings.insert("subFormat".into(), Value::String(format.clone()));
    }
    // Existing conversion jobs retain their keys; remux requests must not reuse them.
    if options.format.as_ref().is_some_and(|format| !format.is_empty()) && !options.audio && !options.recode {
        settings.insert("videoRemux".into(), true.into());
    }
    if options.compatible {
        settings.insert("compatible".into(), true.into());
    }
    for (key, value) in [("filenameTemplate", &options.filename_template), ("folderTemplate", &options.folder_template), ("source", &options.source)] {
        if let Some(value) = value.as_ref().filter(|value| !value.is_empty()) {
            settings.insert(key.into(), Value::String(value.clone()));
        }
    }
    Value::Object(settings)
}

/// Stable key for one video so a re-run with --resume finds the same staging
/// directory. Site ids are untrusted input and never used raw.
pub fn partial_key(metadata: &Metadata, url: &str, options: &Options) -> String {
    digest(&json!({ "source": source_key(metadata, url), "settings": download_settings(options) }))
}

pub fn local_request_key(options: &Options) -> String {
    digest(&json!({
        "url": options.url,
        "entry": options.entry_index.map_or(Value::Null, Value::from),
        "settings": download_settings(options),
        "output": paths::display(paths::resolve(&options.output)),
    }))
}

#[derive(Deserialize, Serialize)]
struct HistoryRecord {
    #[serde(default)]
    version: Value,
    #[serde(default)]
    source: Value,
    #[serde(default)]
    settings: Value,
    #[serde(default)]
    files: Value,
}

fn record_files(record: &HistoryRecord) -> Vec<String> {
    record.files.as_array().map(|files| files.iter().filter_map(|file| file.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// A record can only skip a download while every file it names still exists as
/// a plain file inside the same output directory.
fn history_record_usable(record: Option<&HistoryRecord>, directory: &Path) -> bool {
    let Some(record) = record else { return false };
    let Some(files) = record.files.as_array() else { return false };
    !files.is_empty()
        && files.iter().all(|file| file.as_str().is_some_and(|file| paths::dirname(file) == directory && fs::metadata(file).is_ok_and(|info| info.is_file())))
}

/// Drop a duplicate-detection record whose files are gone, and the history
/// folder with its last record. Returns whether the record is still usable.
fn prune_history_record(history_file: &Path, directory: &Path, history: Option<&HistoryRecord>) -> bool {
    let usable = history_record_usable(history, directory);
    if history.is_some() && !usable {
        let _ = fs::remove_file(history_file);
    }
    if !usable {
        let _ = fs::remove_dir(paths::dirname(history_file));
    }
    usable
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FileRecord {
    source: String,
    destination: String,
}

/// The state file inside a staging directory.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    version: Value,
    #[serde(default)]
    request_key: Value,
    #[serde(default)]
    metadata: Metadata,
    #[serde(default)]
    key: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    source: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    settings: Value,
    #[serde(default)]
    ready: Vec<String>,
    #[serde(default)]
    files: Vec<FileRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backend_succeeded: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compatibility_checked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unconfirmed_output: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<f64>,
}

/// Whether a raw manifest has the shape veo writes.
fn manifest_shape(raw: &Value) -> bool {
    let strings = |value: Option<&Value>| value.and_then(Value::as_array).is_some_and(|items| items.iter().all(Value::is_string));
    let files = raw.get("files").and_then(Value::as_array).is_some_and(|items| {
        items.iter().all(|item| item.get("source").is_some_and(Value::is_string) && item.get("destination").is_some_and(Value::is_string))
    });
    strings(raw.get("ready")) && files
}

/// Read a completed, still usable local transfer without contacting its source.
pub fn read_ready_transfer(options: &Options, local_root: &Path, cancel: &Cancel, cached: Option<&Value>, now: i64) -> Result<Option<Value>> {
    cancel.check()?;
    if options.incognito {
        return Ok(None);
    }
    let request_key = local_request_key(options);
    let staging = paths::resolve(local_root).join(format!("{PARTIAL_PREFIX}{request_key}"));
    let check = || -> Result<Option<Value>> {
        let Some(info) = optional(fs::symlink_metadata(&staging))? else { return Ok(None) };
        if !info.is_dir() || info.file_type().is_symlink() {
            return Ok(None);
        }
        let manifest_file = staging.join("job.json");
        let Some(manifest_info) = optional(fs::symlink_metadata(&manifest_file))? else { return Ok(None) };
        if !manifest_info.is_file() || manifest_info.file_type().is_symlink() {
            return Ok(None);
        }
        let raw: Value = match cached {
            Some(value) => value.clone(),
            None => match state::read_json::<Value>(&manifest_file) {
                Ok(Some(value)) => value,
                Ok(None) => return Ok(None),
                Err(error) if error.is_code("EJSON") => return Ok(None),
                Err(error) => return Err(error),
            },
        };
        let metadata_object = raw.get("metadata").is_some_and(Value::is_object);
        let ready_nonempty = raw.get("ready").and_then(Value::as_array).is_some_and(|ready| !ready.is_empty());
        let files_array = raw.get("files").is_some_and(Value::is_array);
        let succeeded = raw.get("backendSucceeded") == Some(&Value::Bool(true))
            || raw.get("compatibilityChecked") == Some(&Value::Bool(true))
            || raw.get("files").and_then(Value::as_array).is_some_and(|files| !files.is_empty());
        let expiry_ok = match raw.get("expiresAt") {
            None => true,
            Some(value) => value.as_f64().is_some_and(|expires| expires > now as f64),
        };
        if raw.get("version").and_then(Value::as_i64) != Some(1)
            || raw.get("requestKey").and_then(Value::as_str) != Some(request_key.as_str())
            || !metadata_object
            || !ready_nonempty
            || !files_array
            || !succeeded
            || !expiry_ok
            || !manifest_shape(&raw)
        {
            return Ok(None);
        }
        let Ok(manifest) = serde_json::from_value::<Manifest>(raw.clone()) else { return Ok(None) };
        if manifest.key.as_str() != Some(partial_key(&manifest.metadata, &options.url, options).as_str()) {
            return Ok(None);
        }
        let directory = media_destination(options, &manifest.metadata, &staged_title("media.mp4", &manifest.metadata, options.rename.as_deref()))?.directory;
        if manifest.files.iter().any(|item| paths::dirname(&item.source) != staging || paths::dirname(&item.destination) != directory) {
            return Ok(None);
        }
        for file in &manifest.ready {
            if paths::dirname(file) != staging || !is_staged_media(&paths::basename(file)) {
                return Ok(None);
            }
            let media = fs::symlink_metadata(file)?;
            if !media.is_file() || media.file_type().is_symlink() {
                return Ok(None);
            }
        }
        cancel.check()?;
        Ok(Some(raw))
    };
    match check() {
        Ok(value) => Ok(value),
        Err(error) => {
            cancel.check()?;
            if error.not_found() || error.is_code("ENOTDIR") {
                Ok(None)
            } else {
                Err(error)
            }
        }
    }
}

/// Resuming reuses a predictable directory name, so it must not follow a
/// symlink or a file that another process placed there.
fn prepare_staging(partials: &Path) -> Result<PathBuf> {
    if let Err(error) = fs::create_dir(partials) {
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(error.into());
        }
    }
    let info = fs::symlink_metadata(partials).ok();
    if !info.is_some_and(|info| info.is_dir() && !info.file_type().is_symlink()) {
        return Err(Error::msg(format!("The partial download path is not a plain directory: {}", partials.display())));
    }
    Ok(partials.to_path_buf())
}

/// Files a download may leave beside the media file (subtitles, thumbnails).
pub fn is_sidecar(name: &str) -> bool {
    if name.starts_with('.') || is_staged_media(name) {
        return false;
    }
    SIDECAR_EXTENSIONS.contains(&paths::extname(name).trim_start_matches('.').to_lowercase().as_str())
}

/// Title of one staged file. An explicit --rename wins, and collections use the
/// matching entry title so every entry keeps its own name.
pub fn staged_title(file: &str, metadata: &Metadata, rename: Option<&str>) -> String {
    let index = staged_index(&paths::basename(file));
    let fallback = first(&[&metadata.title, &metadata.id]).unwrap_or("video").to_string();
    let title = match &index {
        Some(index) => {
            let position: usize = index.parse().unwrap_or(0);
            metadata
                .entries
                .as_ref()
                .and_then(|entries| entries.get(position.wrapping_sub(1)))
                .and_then(|entry| entry.as_ref())
                .and_then(|entry| first(&[&entry.title]).map(str::to_string))
                .unwrap_or_else(|| format!("{fallback} - {index}"))
        }
        None => fallback,
    };
    match rename.filter(|rename| !rename.is_empty()) {
        Some(rename) if rename.contains('*') => rename.replace('*', &title),
        Some(rename) => match index {
            Some(index) => format!("{rename} - {index}"),
            None => rename.to_string(),
        },
        None => title,
    }
}

fn s(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

fn backend_args(options: &Options, backend: &Backend) -> Vec<String> {
    let mut args =
        s(&["--ignore-config", "--no-plugin-dirs", "--no-colors", "--no-warnings", "--socket-timeout", "30", "--retries", "3", "--fragment-retries", "3"]);
    if let Some((name, path)) = backend::js_runtime() {
        args.extend(["--no-js-runtimes".to_string(), "--js-runtimes".to_string(), format!("{name}:{}", path.display())]);
    }
    args.extend(["--ffmpeg-location".to_string(), paths::display(&backend.ffmpeg_location)]);
    if options.incognito {
        args.push("--no-cache-dir".into());
    }
    if !options.playlist {
        args.push("--no-playlist".into());
    }
    if let Some(index) = options.entry_index {
        args.extend(["--playlist-items".into(), index.to_string()]);
    }
    args.extend(["--concurrent-fragments".into(), options.concurrent_fragments.to_string()]);
    // Credentials are opt-in per invocation and only forwarded to the backend.
    if let Some(cookies) = &options.cookies {
        args.extend(["--cookies".into(), cookies.clone()]);
    }
    if let Some(browser) = &options.cookies_from_browser {
        args.extend(["--cookies-from-browser".into(), browser.clone()]);
    }
    if !options.url.is_empty() {
        if options.media_url.is_some() {
            args.extend(["--referer".into(), options.url.clone()]);
        } else if let Ok(url) = url::Url::parse(&options.url) {
            args.extend(["--referer".into(), format!("{}/", url.origin().ascii_serialization())]);
        }
    }
    args
}

fn media_args(options: &Options, quality: &QualityChoice) -> Vec<String> {
    let mut args = vec![
        "--no-overwrites".to_string(),
        if options.resume { "--continue" } else { "--no-continue" }.into(),
        "--newline".into(),
        "--progress".into(),
        "--progress-delta".into(),
        "0.2".into(),
    ];
    args.extend(s(&[
        "--progress-template",
        r#"download:veo-progress:{"progress":%(progress)j,"video":%(info.vcodec|null)j,"audio":%(info.acodec|null)j}"#,
        "--progress-template",
        "postprocess:veo-postprocess:%(progress)j",
        "--print",
        "after_move:veo-file:%(filepath)j",
        "--no-simulate",
    ]));
    if options.audio {
        args.extend(s(&["-f", "ba/b", "--extract-audio", "--audio-format", options.format.as_deref().unwrap_or("mp3"), "--audio-quality", "0"]));
    } else {
        if let Some(format) = &quality.format {
            args.extend(["-f".into(), format.clone()]);
        }
        // Merge into a permissive container first; e.g. H.264 cannot be merged
        // straight into WebM before the requested codec conversion runs.
        if options.compatible {
            args.extend(s(&["--merge-output-format", "mkv"]));
        } else if let Some(format) = &options.format {
            args.extend(s(&["--merge-output-format", "mkv", if options.recode { "--recode-video" } else { "--remux-video" }, format]));
        } else {
            args.extend(s(&["--merge-output-format", "mp4/mkv"]));
        }
    }
    let sort = ["vcodec:h264,acodec:aac".to_string()].into_iter().chain(quality.sort.clone()).collect::<Vec<_>>().join(",");
    // Prefer broadly playable sources; explicit recoding and WebM keep their own choice.
    let selected = if options.recode || options.format.as_deref() == Some("webm") { quality.sort.clone() } else { Some(sort) };
    if let (false, Some(selected)) = (options.audio, selected) {
        args.extend(["-S".into(), selected]);
    }
    if options.subs || options.auto_subs || options.sub_langs.is_some() || options.embed_subs {
        if options.subs || (options.embed_subs && !options.auto_subs) {
            args.push("--write-subs".into());
        }
        if options.auto_subs {
            args.push("--write-auto-subs".into());
        }
        args.extend(["--sub-langs".into(), options.sub_langs.clone().filter(|langs| !langs.is_empty()).unwrap_or_else(|| "en.*,en".into())]);
    }
    if let Some(format) = options.sub_format.as_ref().filter(|format| !format.is_empty()) {
        args.extend(["--sub-format".into(), format.clone()]);
    }
    if options.embed_subs {
        args.push("--embed-subs".into());
    }
    if options.embed_metadata {
        args.push("--embed-metadata".into());
    }
    if options.embed_thumbnail {
        args.push("--embed-thumbnail".into());
    }
    if let Some(categories) = options.sponsorblock_remove.as_ref().filter(|value| !value.is_empty()) {
        args.extend(["--sponsorblock-remove".into(), categories.clone()]);
    }
    if let Some(section) = options.section.as_ref().filter(|value| !value.is_empty()) {
        args.extend(["--download-sections".into(), section.clone(), "--force-keyframes-at-cuts".into()]);
    }
    args
}

pub fn staging_template(directory: &Path, playlist: bool) -> String {
    let name = if playlist { "media-%(playlist_index)03d.%(ext)s" } else { "media.%(ext)s" };
    paths::display(directory.join(name)).replace('%', "%%").replacen("%%(playlist_index)03d", "%(playlist_index)03d", 1).replacen("%%(ext)s", "%(ext)s", 1)
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
    args.push(options.media_url.clone().unwrap_or_else(|| options.url.clone()));
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
fn announce(reporter: &dyn Reporter, metadata: Metadata) -> Metadata {
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

fn single(options: &Options) -> Options {
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

fn estimate_json(estimate: &Estimate, map: &mut Map<String, Value>) {
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

/// Outcome of one URL (or one playlist).
#[derive(Clone, Debug, Default)]
pub struct DownloadResult {
    pub url: String,
    pub title: Option<String>,
    pub files: Vec<String>,
    pub status: String,
    pub saved: u64,
    pub skipped: u64,
    pub failures: Option<Vec<Value>>,
    pub verified: bool,
    pub timings: Option<Map<String, Value>>,
    pub entry_timings: Vec<Value>,
}

impl DownloadResult {
    /// JSON in the field order veo has always printed.
    pub fn to_json(&self) -> Map<String, Value> {
        let mut map = Map::new();
        map.insert("url".into(), Value::String(self.url.clone()));
        if let Some(title) = &self.title {
            map.insert("title".into(), Value::String(title.clone()));
        }
        map.insert("files".into(), json!(self.files));
        if self.failures.is_none() {
            map.insert("status".into(), Value::String(self.status.clone()));
        }
        map.insert("saved".into(), self.saved.into());
        map.insert("skipped".into(), self.skipped.into());
        if let Some(failures) = &self.failures {
            map.insert("failures".into(), Value::Array(failures.clone()));
        }
        if self.verified {
            map.insert("verified".into(), true.into());
        }
        if let Some(timings) = &self.timings {
            map.insert("timings".into(), Value::Object(timings.clone()));
        }
        if !self.entry_timings.is_empty() {
            map.insert("entryTimings".into(), Value::Array(self.entry_timings.clone()));
        }
        if self.failures.is_some() {
            map.insert("status".into(), Value::String(self.status.clone()));
        }
        map
    }
}

/// Per-entry progress of a playlist, reported as each entry finishes.
#[derive(Clone, Debug)]
pub struct EntryOutcome {
    pub index: u64,
    pub status: String,
    pub files: Vec<String>,
    pub error: Option<String>,
    pub timings: Option<Map<String, Value>>,
}

impl EntryOutcome {
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("index".into(), self.index.into());
        map.insert("status".into(), Value::String(self.status.clone()));
        if let Some(error) = &self.error {
            map.insert("error".into(), Value::String(error.clone()));
        }
        map.insert("files".into(), json!(self.files));
        if let Some(timings) = &self.timings {
            map.insert("timings".into(), Value::Object(timings.clone()));
        }
        Value::Object(map)
    }
}

pub type Verifier<'a> = dyn Fn(&Path, bool, &Cancel) -> Result<()> + Sync + 'a;
pub type DiskChecker<'a> = dyn Fn(&Path, &Path, Option<f64>, bool, &dyn Reporter) -> Result<Reservation> + Sync + 'a;
pub type Wait<'a> = dyn Fn(Duration, &Cancel) -> Result<()> + Sync + 'a;

pub fn default_verifier(file: &Path, audio: bool, cancel: &Cancel) -> Result<()> {
    crate::inspect_media::verify_saved_media(file, audio, cancel).map(|_| ())
}

pub fn default_disk_checker(cache: &Path, destination: &Path, bytes: Option<f64>, cached: bool, reporter: &dyn Reporter) -> Result<Reservation> {
    reserve_space(cache, destination, bytes, cached, reporter)
}

/// Everything a download needs besides its options; tests replace the tools.
pub struct Deps<'a> {
    pub cancel: &'a Cancel,
    pub reporter: Arc<dyn Reporter>,
    pub backend: &'a BackendResolver<'a>,
    pub runner: &'a Runner<'a>,
    pub compatibility_runner: &'a Runner<'a>,
    pub local_root: PathBuf,
    pub skip_cache_cleanup: bool,
    pub adaptive: &'a Adaptive,
    pub wait: &'a Wait<'a>,
    pub disk_checker: &'a DiskChecker<'a>,
    pub verifier: &'a Verifier<'a>,
    pub on_entry: Option<&'a (dyn Fn(&EntryOutcome) -> Result<()> + Sync)>,
}

impl Deps<'_> {
    fn with_reporter(&self, reporter: Arc<dyn Reporter>) -> Deps<'_> {
        Deps {
            cancel: self.cancel,
            reporter,
            backend: self.backend,
            runner: self.runner,
            compatibility_runner: self.compatibility_runner,
            local_root: self.local_root.clone(),
            skip_cache_cleanup: true,
            adaptive: self.adaptive,
            wait: self.wait,
            disk_checker: self.disk_checker,
            verifier: self.verifier,
            on_entry: self.on_entry,
        }
    }
}

fn download_collection(options: &Options, metadata: &Metadata, deps: &Deps, backend: &Backend) -> Result<DownloadResult> {
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
        for worker in 0..(concurrency as usize).min(entries.len()) {
            let (entries, results, failures, entry_timings, saved, skipped, notify, next, fatal, child_deps, reporter, playlist_title) =
                (&entries, &results, &failures, &entry_timings, &saved, &skipped, &notify, &next, &fatal, &child_deps, &reporter, &playlist_title);
            scope.spawn(move || {
                while next.load(Ordering::SeqCst) < entries.len() && (worker as u32) < deps.adaptive.reduced(concurrency) {
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
                    let item_options = Options { entry_index: Some(index), playlist_title: playlist_title.clone(), rename, ..options.clone() };
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

fn unique(files: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    files.iter().filter(|file| seen.insert((*file).clone())).cloned().collect()
}

fn verify_files(files: &[String], options: &Options, deps: &Deps) -> Result<()> {
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
fn save_sidecars(
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

#[cfg(test)]
mod tests {
    use super::*;

    fn heights(values: &[f64]) -> Vec<Format> {
        values.iter().map(|height| Format { height: Some(*height), vcodec: Some("avc1".into()), ..Format::default() }).collect()
    }

    #[test]
    fn quality_is_an_upper_bound() {
        let formats = heights(&[2160.0, 1080.0, 720.0]);
        let choice = select_quality("1440p", false, false, &formats, false);
        assert_eq!(choice.format.as_deref(), Some("bv[height=1080]+ba/b[height=1080]/bv[height=1080]"));
        assert_eq!(choice.label.as_deref(), Some("Quality: 1080p (highest at or below 1440p)"));
        let none = select_quality("480p", false, false, &formats, false);
        assert!(none.error.unwrap().contains("offered: 720p, 1080p, 2160p"));
        let closest = select_quality("1440p", false, true, &formats, false);
        assert_eq!(closest.label.as_deref(), Some("Quality: 1080p (closest to 1440p)"));
        assert_eq!(select_quality("720p", false, false, &formats, true).sort.as_deref(), Some("res:720"));
        assert_eq!(select_quality("720p", true, false, &formats, false), QualityChoice::default());
        assert_eq!(select_quality("720p", false, false, &[], false).label.as_deref(), Some("Resolution unknown; using the best available stream."));
    }

    #[test]
    fn recognizes_staged_files() {
        assert!(is_staged_media("media.mp4"));
        assert!(is_staged_media("media-002.webm"));
        assert!(!is_staged_media("media.f137.mp4"));
        assert!(!is_staged_media("media.jpg"));
        assert!(is_sidecar("media.en.vtt"));
        assert!(!is_sidecar("media.mp4"));
        assert!(!is_sidecar(".lock"));
    }

    #[test]
    fn titles_follow_rename_rules() {
        let metadata = Metadata {
            title: Some("List".into()),
            entries: Some(vec![Some(Metadata { title: Some("First".into()), ..Metadata::default() }), None]),
            ..Metadata::default()
        };
        assert_eq!(staged_title("media-001.mp4", &metadata, None), "First");
        assert_eq!(staged_title("media-002.mp4", &metadata, None), "List - 002");
        assert_eq!(staged_title("media.mp4", &metadata, Some("x *")), "x List");
        assert_eq!(staged_title("media-001.mp4", &metadata, Some("Name")), "Name - 001");
    }

    #[test]
    fn staging_template_escapes_percent() {
        let template = staging_template(Path::new("/tmp/a%b"), true);
        assert!(template.ends_with("a%%b/media-%(playlist_index)03d.%(ext)s") || template.ends_with("a%%b\\media-%(playlist_index)03d.%(ext)s"), "{template}");
    }

    #[test]
    fn settings_keys_are_stable() {
        let options = Options { quality: "best".into(), url: "https://example.com/v".into(), output: "out".into(), ..Options::default() };
        assert_eq!(
            serde_json::to_string(&download_settings(&options)).unwrap(),
            r#"{"quality":"best","audio":false,"format":null,"closestQuality":false,"subs":false,"subLangs":null,"embedSubs":false,"embedMetadata":false,"embedThumbnail":false,"sponsorblockRemove":null,"section":null}"#
        );
        let metadata = Metadata { id: Some("abc".into()), extractor_key: Some("Youtube".into()), ..Metadata::default() };
        assert_eq!(partial_key(&metadata, &options.url, &options).len(), 24);
    }

    #[test]
    fn staging_keys_match_veo_1x() {
        // Values computed with the JavaScript implementation (veo 1.12.1), so
        // retained and resumable downloads of 1.x stay usable after upgrading.
        let args: Vec<String> = ["https://example.com/watch?v=1", "-q", "720p", "-o", "C:/out", "--subs", "--sub-format", "srt", "--format", "mkv"]
            .iter()
            .map(|arg| arg.to_string())
            .collect();
        let options = crate::cli::parse_cli(&args, &Map::new()).unwrap();
        let metadata = Metadata { id: Some("abc".into()), extractor_key: Some("Youtube".into()), ..Metadata::default() };
        assert_eq!(partial_key(&metadata, &options.url, &options), "79f4859f93e33b59eb790ca9");
        if cfg!(windows) {
            assert_eq!(local_request_key(&options), "db4dee629cd61e627a57b5c8");
            assert_eq!(local_request_key(&Options { entry_index: Some(3), ..options }), "fdb1c800cea8bd69b8cffc65");
        }
    }

    #[test]
    fn result_json_keeps_field_order() {
        let result = DownloadResult { url: "u".into(), title: Some("t".into()), files: vec![], status: "saved".into(), saved: 1, ..DownloadResult::default() };
        assert_eq!(serde_json::to_string(&result.to_json()).unwrap(), r#"{"url":"u","title":"t","files":[],"status":"saved","saved":1,"skipped":0}"#);
    }
}
