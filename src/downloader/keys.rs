//! Stable keys for staging directories and duplicate-detection records.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::metadata::{first, Metadata};
use crate::options::Options;
use crate::paths;
use crate::state::digest;

pub(super) fn source_key(metadata: &Metadata, url: &str) -> String {
    match (first(&[&metadata.id]), first(&[&metadata.extractor_key, &metadata.extractor])) {
        (Some(id), Some(extractor)) => format!("{extractor}:{id}"),
        _ => format!("{url}#{}", metadata.id.as_deref().unwrap_or("")),
    }
}

pub(super) fn text_or_null(value: &Option<String>) -> Value {
    value.as_ref().map_or(Value::Null, |text| Value::String(text.clone()))
}

/// Settings that change the produced file; part of every staging key.
pub(super) fn download_settings(options: &Options) -> Value {
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
pub(super) struct HistoryRecord {
    #[serde(default)]
    pub(super) version: Value,
    #[serde(default)]
    pub(super) source: Value,
    #[serde(default)]
    pub(super) settings: Value,
    #[serde(default)]
    pub(super) files: Value,
}

pub(super) fn record_files(record: &HistoryRecord) -> Vec<String> {
    record.files.as_array().map(|files| files.iter().filter_map(|file| file.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// A record can only skip a download while every file it names still exists as
/// a plain file inside the same output directory.
pub(super) fn history_record_usable(record: Option<&HistoryRecord>, directory: &Path) -> bool {
    let Some(record) = record else { return false };
    let Some(files) = record.files.as_array() else { return false };
    !files.is_empty()
        && files.iter().all(|file| file.as_str().is_some_and(|file| paths::dirname(file) == directory && fs::metadata(file).is_ok_and(|info| info.is_file())))
}

/// Drop a duplicate-detection record whose files are gone, and the history
/// folder with its last record. Returns whether the record is still usable.
pub(super) fn prune_history_record(history_file: &Path, directory: &Path, history: Option<&HistoryRecord>) -> bool {
    let usable = history_record_usable(history, directory);
    if history.is_some() && !usable {
        let _ = fs::remove_file(history_file);
    }
    if !usable {
        let _ = fs::remove_dir(paths::dirname(history_file));
    }
    usable
}
