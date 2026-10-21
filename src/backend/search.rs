//! Finding trusted executables on PATH and in well-known directories.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::fsutil::is_executable;
use crate::paths::{self, platform};

/// Directories where Git for Windows, WinGet, Homebrew and distributions place
/// ffmpeg without always extending the inherited PATH.
pub fn well_known_media_directories(platform: &str) -> Vec<PathBuf> {
    if platform == "android" {
        return std::env::var_os("PREFIX").map(|prefix| vec![PathBuf::from(prefix).join("bin")]).unwrap_or_default();
    }
    if platform != "win32" {
        return ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"].iter().map(PathBuf::from).collect();
    }
    let local = std::env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()).map(PathBuf::from);
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    let mut directories = Vec::new();
    if let Some(local) = &local {
        directories.push(local.join("Microsoft").join("WinGet").join("Links"));
        directories.push(local.join("Microsoft").join("WindowsApps"));
    }
    directories.push(program_files.join("ffmpeg").join("bin"));
    directories.push(PathBuf::from(r"C:\ffmpeg\bin"));
    directories.push(PathBuf::from(r"C:\ProgramData\chocolatey\bin"));
    directories
}

pub(super) fn normalized(file: &Path) -> String {
    let resolved = paths::display(paths::resolve(file));
    if cfg!(windows) {
        resolved.to_lowercase()
    } else {
        resolved
    }
}

pub(super) fn canonical(path: &Path) -> Option<PathBuf> {
    fs::canonicalize(path).ok()
}

pub fn excluded_path(file: Option<&Path>, exclude: &[PathBuf]) -> bool {
    let Some(file) = file else { return false };
    if exclude.is_empty() {
        return false;
    }
    let resolved = normalized(file);
    if exclude.iter().any(|excluded| normalized(excluded) == resolved) {
        return true;
    }
    let canonical_file = canonical(file);
    let entry = canonical(&paths::dirname(file)).map(|parent| parent.join(paths::basename(file)));
    for excluded in exclude {
        let Some(directory) = canonical(&paths::dirname(excluded)) else { continue };
        // Resolve directory aliases, but do not follow a rejected cache file's
        // outward symlink and accidentally blacklist its genuine system target.
        let regular = fs::symlink_metadata(excluded).ok().filter(fs::Metadata::is_file).and_then(|_| canonical(excluded));
        let target = normalized(&regular.unwrap_or_else(|| directory.join(paths::basename(excluded))));
        if entry.as_ref().is_some_and(|entry| normalized(entry) == target) || canonical_file.as_ref().is_some_and(|file| normalized(file) == target) {
            return true;
        }
    }
    false
}

/// First matching executable on PATH (plus well-known media directories).
pub fn find_on_path(names: &[&str], directories: Option<&[PathBuf]>, exclude: &[PathBuf]) -> Option<PathBuf> {
    let platform = platform();
    let candidates: Vec<PathBuf> = match directories {
        Some(directories) => directories.to_vec(),
        None => {
            let raw = std::env::var_os("PATH").or_else(|| std::env::var_os("Path")).unwrap_or_default();
            std::env::split_paths(&raw)
                .map(|entry| {
                    let text = entry.to_string_lossy().trim().to_string();
                    PathBuf::from(text.strip_prefix('"').and_then(|text| text.strip_suffix('"')).unwrap_or(&text))
                })
                .chain(well_known_media_directories(platform))
                .collect()
        }
    };
    let suffixes: &[&str] = if platform == "win32" { &["", ".exe", ".cmd"] } else { &[""] };
    for directory in candidates.iter().filter(|directory| directory.is_absolute()) {
        for name in names {
            for suffix in suffixes {
                let candidate = directory.join(format!("{name}{suffix}"));
                if is_executable(&candidate) && !excluded_path(Some(&candidate), exclude) {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

pub fn find(names: &[&str]) -> Option<PathBuf> {
    find_on_path(names, None, &[])
}

pub(super) fn env_path(name: &str) -> Result<Option<PathBuf>> {
    let Some(value) = std::env::var_os(name) else { return Ok(None) };
    let text = value.to_string_lossy();
    if text.trim().is_empty() || text.contains('\0') {
        return Err(Error::msg(format!("{name} must be a non-empty filesystem path.")));
    }
    Ok(Some(paths::resolve(&value)))
}

pub(super) fn executable(file: &Path, label: &str) -> Result<PathBuf> {
    if !is_executable(file) {
        return Err(Error::msg(format!("{label} is missing or not executable: {}", file.display())));
    }
    Ok(paths::resolve(file))
}
