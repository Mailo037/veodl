//! The local staging directory: manifest, staged files and completed transfers.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cancel::Cancel;
use crate::error::{optional, Error, Result};
use crate::metadata::{first, Metadata};
use crate::naming::media_destination;
use crate::options::Options;
use crate::paths;
use crate::state;

use super::*;

pub(super) const PARTIAL_PREFIX: &str = ".veo-part-";

// Only real media extensions may count as the downloaded file, so a thumbnail
// or subtitle the backend leaves behind is never mistaken for it.
pub(super) const MEDIA_EXTENSIONS: [&str; 23] = [
    "mp4", "mkv", "webm", "mov", "avi", "m4v", "3gp", "ts", "flv", "mpg", "mpeg", "ogv", "mp3", "m4a", "aac", "opus", "flac", "wav", "ogg", "oga", "wma",
    "mka", "aiff",
];

// Formats the backend may leave beside the media file. Merge fragments such as
// media.f137.mp4 are never deliverables.
pub(super) const SIDECAR_EXTENSIONS: [&str; 13] = ["vtt", "srt", "ass", "lrc", "ttml", "srv1", "srv2", "srv3", "json", "jpg", "jpeg", "png", "webp"];

pub fn is_staged_media(name: &str) -> bool {
    re!(r"^media(?:-([0-9]+))?\.([A-Za-z0-9]{1,8})$").captures(name).is_some_and(|captures| MEDIA_EXTENSIONS.contains(&captures[2].to_lowercase().as_str()))
}

pub(super) fn staged_index(name: &str) -> Option<String> {
    if !is_staged_media(name) {
        return None;
    }
    re!(r"^media(?:-([0-9]+))?\.").captures(name).and_then(|captures| captures.get(1).map(|index| index.as_str().to_string()))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct FileRecord {
    pub(super) source: String,
    pub(super) destination: String,
}

/// The state file inside a staging directory.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Manifest {
    pub(super) version: Value,
    #[serde(default)]
    pub(super) request_key: Value,
    #[serde(default)]
    pub(super) metadata: Metadata,
    #[serde(default)]
    pub(super) key: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub(super) source: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub(super) settings: Value,
    #[serde(default)]
    pub(super) ready: Vec<String>,
    #[serde(default)]
    pub(super) files: Vec<FileRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) backend_succeeded: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) compatibility_checked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) unconfirmed_output: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) expires_at: Option<f64>,
}

/// Whether a raw manifest has the shape veo writes.
pub(super) fn manifest_shape(raw: &Value) -> bool {
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
pub(super) fn prepare_staging(partials: &Path) -> Result<PathBuf> {
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

pub(super) fn unique(files: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    files.iter().filter(|file| seen.insert((*file).clone())).cloned().collect()
}
