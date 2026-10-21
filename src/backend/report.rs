//! Read-only backend inspection for veo doctor.

use std::fs;
use std::path::PathBuf;

use serde::Serialize;

use crate::cancel::Cancel;
use crate::error::Result;
use crate::fsutil::is_executable;
use crate::paths::{arch, exe_suffix, platform};
use crate::tool_setup;

use super::*;

#[derive(Clone, Debug, Default, Serialize)]
pub struct ToolState {
    pub source: String,
    pub path: Option<PathBuf>,
    pub present: bool,
    pub verified: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct BackendReport {
    pub release: String,
    pub directory: PathBuf,
    pub platform: String,
    pub asset: Option<String>,
    pub override_: Option<ActiveOverride>,
    pub yt_dlp: ToolState,
    pub ffmpeg: ToolState,
    pub ffprobe: ToolState,
    pub errors: Vec<String>,
}

/// Report the state of every backend component without downloading, executing,
/// or changing anything. Used by `veo doctor`.
pub fn inspect_backend(cancel: &Cancel) -> BackendReport {
    let suffix = exe_suffix();
    let directory = cache_directory(RELEASE);
    let (platform, arch) = (platform(), arch());
    let mut report = BackendReport {
        release: RELEASE.into(),
        directory: directory.clone(),
        platform: format!("{platform}/{arch}"),
        yt_dlp: ToolState { source: "none".into(), verified: Some(false), ..ToolState::default() },
        ffmpeg: ToolState { source: "missing".into(), ..ToolState::default() },
        ffprobe: ToolState { source: "missing".into(), ..ToolState::default() },
        ..BackendReport::default()
    };
    let yt = (|| -> Result<ToolState> {
        if let Some(path) = env_path("VEO_YT_DLP_PATH")? {
            return Ok(ToolState { source: "override".into(), present: is_executable(&path), path: Some(path), verified: Some(false) });
        }
        if platform == "android" {
            let file = find(&["yt-dlp"]);
            return Ok(ToolState { source: "system".into(), present: file.is_some(), path: file, verified: Some(false) });
        }
        let asset = select_asset(platform, arch, musl());
        report.asset = asset.clone();
        let Some(asset) = asset else {
            report.errors.push(format!("No standalone yt-dlp is published for {}.", report.platform));
            return Ok(report.yt_dlp.clone());
        };
        report.override_ = active_override(Some(&asset), cancel, &backend_state_file())?;
        let file = report.override_.as_ref().map(|active| active.path.clone()).unwrap_or_else(|| directory.join(format!("yt-dlp{suffix}")));
        let present = is_executable(&file);
        let expected = report.override_.as_ref().map(|active| active.sha256.clone()).or_else(|| pinned_hash(&asset).map(str::to_string)).unwrap_or_default();
        let verified = present && matches(&file, &expected, cancel)?;
        Ok(ToolState { source: if report.override_.is_some() { "installed" } else { "managed" }.into(), path: Some(file), present, verified: Some(verified) })
    })();
    match yt {
        Ok(state) => report.yt_dlp = state,
        Err(error) => {
            report.errors.push(error.message);
            report.yt_dlp = ToolState { source: "invalid".into(), verified: Some(false), ..ToolState::default() };
        }
    }
    let media_override = match env_path("VEO_FFMPEG_PATH") {
        Ok(value) => value,
        Err(error) => {
            report.errors.push(error.message);
            None
        }
    };
    let cache_verified =
        platform != "android" && media_override.is_none() && tool_setup::verified_media_tools(&directory, platform, arch, cancel).unwrap_or(false);
    let exclude: Vec<PathBuf> =
        if platform == "android" { Vec::new() } else { ["ffmpeg", "ffprobe"].iter().map(|name| directory.join(format!("{name}{suffix}"))).collect() };
    for name in ["ffmpeg", "ffprobe"] {
        let cached = directory.join(format!("{name}{suffix}"));
        let entry = if let Some(location) = &media_override {
            let file = location.join(format!("{name}{suffix}"));
            ToolState { source: "override".into(), present: is_executable(&file), path: Some(file), verified: None }
        } else if cache_verified {
            ToolState { source: "cache".into(), present: is_executable(&cached), path: Some(cached), verified: Some(true) }
        } else {
            let found = find_on_path(&[name], None, &exclude).filter(|found| !excluded_path(Some(found), &exclude));
            match found {
                Some(found) => ToolState { source: "path".into(), path: Some(found), present: true, verified: Some(false) },
                None if platform != "android" && fs::symlink_metadata(&cached).is_ok() => {
                    ToolState { source: "cache".into(), path: Some(cached), present: false, verified: Some(false) }
                }
                None => ToolState { source: "missing".into(), path: None, present: false, verified: Some(false) },
            }
        };
        if name == "ffmpeg" {
            report.ffmpeg = entry;
        } else {
            report.ffprobe = entry;
        }
    }
    report
}
