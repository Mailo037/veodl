//! An explicitly installed newer yt-dlp release.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cancel::Cancel;
use crate::error::{Error, Result};
use crate::paths::{arch, exe_suffix, platform};
use crate::state;

use super::*;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct OverrideState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed: Option<String>,
}

pub fn read_backend_override(file: &Path) -> Option<OverrideState> {
    state::read_json_lenient(file)
}

pub fn write_backend_override(value: &OverrideState, file: &Path) -> Result<()> {
    state::write_json(file, value)
}

/// Returns true only when a recorded override was actually removed.
pub fn clear_backend_override(file: &Path) -> Result<bool> {
    match fs::remove_file(file) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ActiveOverride {
    pub release: String,
    pub asset: String,
    pub sha256: String,
    pub path: PathBuf,
}

/// The explicitly installed backend only wins when it is newer than the pinned
/// release, matches this platform's asset, and still hashes to the recorded
/// value. Anything else silently falls back to the pinned release.
pub fn active_override(asset: Option<&str>, cancel: &Cancel, file: &Path) -> Result<Option<ActiveOverride>> {
    let Some(asset) = asset else { return Ok(None) };
    let Some(state) = read_backend_override(file) else { return Ok(None) };
    let (Some(release), Some(sha256)) = (state.release, state.sha256) else { return Ok(None) };
    if state.asset.as_deref() != Some(asset) || !is_valid_release(&release) || !re!(r"^[a-f0-9]{64}$").is_match(&sha256) {
        return Ok(None);
    }
    if crate::version::cmp(&release, RELEASE) != std::cmp::Ordering::Greater {
        return Ok(None);
    }
    let path = cache_directory(&release).join(format!("yt-dlp{}", exe_suffix()));
    if !matches(&path, &sha256, cancel)? {
        return Ok(None);
    }
    Ok(Some(ActiveOverride { release, asset: asset.to_string(), sha256, path }))
}

/// Install a backend release newer than the pinned one. The expected hash must
/// come from that release's own checksum list.
pub fn install_backend(release: &str, asset: Option<&str>, sha256: &str, cancel: &Cancel, status: &dyn Fn(&str)) -> Result<PathBuf> {
    if !is_valid_release(release) {
        return Err(Error::msg(format!("Invalid yt-dlp release: {release}")));
    }
    let asset = asset.ok_or_else(|| Error::msg(format!("No standalone yt-dlp is available for {}/{}.", platform(), arch())))?;
    if !re!(r"^[a-f0-9]{64}$").is_match(sha256) {
        return Err(Error::msg("The release checksum is missing or malformed."));
    }
    let directory = cache_directory(release);
    fs::create_dir_all(&directory)?;
    download_backend(asset, release, sha256, &directory, cancel, status, true)
}
