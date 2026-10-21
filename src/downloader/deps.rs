//! Results and injectable dependencies of a download.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::cancel::Cancel;
use crate::disk_space::{reserve_space, Reservation};
use crate::error::Result;
use crate::execution::Adaptive;
use crate::progress::Reporter;

use super::*;

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
    pub(super) fn with_reporter(&self, reporter: Arc<dyn Reporter>) -> Deps<'_> {
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
