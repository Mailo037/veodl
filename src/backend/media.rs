//! Locating or installing the FFmpeg/FFprobe pair.

use std::fs;
use std::path::{Path, PathBuf};

use crate::cancel::Cancel;
use crate::error::{Error, Result};
use crate::paths::{self, arch, exe_suffix, platform};
use crate::state;
use crate::tool_setup;

use super::*;

pub(super) fn managed_media_tools(directory: &Path, cancel: &Cancel) -> Result<Option<&'static str>> {
    if !tool_setup::verified_media_tools(directory, platform(), arch(), cancel)? {
        return Ok(Some("the managed media tools are missing or failed SHA-256 verification"));
    }
    if !cfg!(windows) {
        for name in ["ffmpeg", "ffprobe"] {
            state::set_mode(&directory.join(name), 0o755)?;
        }
    }
    Ok(None)
}

pub struct MediaHooks<'a> {
    pub find: &'a dyn Fn(&[&str], &[PathBuf]) -> Option<PathBuf>,
    pub managed: &'a dyn Fn(&Path, &Cancel) -> Result<Option<&'static str>>,
    pub install: &'a dyn Fn(&Path, &Cancel, &dyn Fn(&str)) -> Result<PathBuf>,
}

pub(super) fn default_find(names: &[&str], exclude: &[PathBuf]) -> Option<PathBuf> {
    find_on_path(names, None, exclude)
}

pub(super) fn default_install(directory: &Path, cancel: &Cancel, status: &dyn Fn(&str)) -> Result<PathBuf> {
    tool_setup::install_media_tools(directory, cancel, status, platform(), arch())
}

impl Default for MediaHooks<'_> {
    fn default() -> Self {
        MediaHooks { find: &default_find, managed: &managed_media_tools, install: &default_install }
    }
}

/// Resolve a location that contains both ffmpeg and ffprobe for
/// --ffmpeg-location. Precedence: explicit override, verified managed cache,
/// then a system installation. Missing desktop tools use pinned downloads.
pub fn resolve_media_tools(
    directory: &Path,
    offline: bool,
    cancel: &Cancel,
    status: &dyn Fn(&str),
    status_done: &dyn Fn(&str, &str),
    hooks: &MediaHooks,
) -> Result<PathBuf> {
    let preparing = "Preparing ffmpeg and ffprobe…";
    let current = std::cell::RefCell::new(String::new());
    let report = |message: &str| {
        *current.borrow_mut() = message.to_string();
        status(message);
    };
    let finish = |outcome: &str| {
        if *current.borrow() != preparing {
            report(preparing);
        }
        status_done(preparing, outcome);
    };
    report(preparing);
    let result = (|| -> Result<PathBuf> {
        let suffix = exe_suffix();
        if let Some(location) = env_path("VEO_FFMPEG_PATH")? {
            executable(&location.join(format!("ffmpeg{suffix}")), "VEO_FFMPEG_PATH ffmpeg")?;
            executable(&location.join(format!("ffprobe{suffix}")), "VEO_FFMPEG_PATH ffprobe")?;
            finish("done");
            report("Using VEO_FFMPEG_PATH override.");
            return Ok(location);
        }
        let Some(problem) = (hooks.managed)(directory, cancel)? else {
            finish("done");
            return Ok(directory.to_path_buf());
        };
        let exclude: Vec<PathBuf> = ["ffmpeg", "ffprobe"].iter().map(|name| directory.join(format!("{name}{suffix}"))).collect();
        let system = |name: &str| (hooks.find)(&[name], &exclude).filter(|found| !excluded_path(Some(found), &exclude));
        let (ffmpeg, ffprobe) = (system("ffmpeg"), system("ffprobe"));
        let (Some(ffmpeg), Some(ffprobe)) = (ffmpeg.clone(), ffprobe.clone()) else {
            if !offline {
                let location = (hooks.install)(directory, cancel, &report)?;
                finish("done");
                return Ok(location);
            }
            let missing =
                [ffmpeg.is_none().then_some("ffmpeg"), ffprobe.is_none().then_some("ffprobe")].into_iter().flatten().collect::<Vec<_>>().join(" and ");
            return Err(Error::msg(format!(
                "Cannot load media tools: {problem}, and no {missing} was found on PATH. Run veo doctor fix without --offline to install them automatically, or set VEO_FFMPEG_PATH to a directory containing both binaries."
            )));
        };
        // Same directory: yt-dlp can use it in place instead of duplicating ~150 MB.
        if paths::dirname(&ffmpeg) == paths::dirname(&ffprobe) {
            finish("done");
            report(&format!("Using the system ffmpeg in {}.", paths::dirname(&ffmpeg).display()));
            return Ok(paths::dirname(&ffmpeg));
        }
        report("Copying system ffmpeg and ffprobe into the backend cache…");
        fs::create_dir_all(directory)?;
        stage(&ffmpeg, &directory.join(format!("ffmpeg{suffix}")), cancel)?;
        stage(&ffprobe, &directory.join(format!("ffprobe{suffix}")), cancel)?;
        finish("done");
        Ok(directory.to_path_buf())
    })();
    if result.is_err() && !cancel.is_cancelled() {
        finish("failed");
    }
    result
}
