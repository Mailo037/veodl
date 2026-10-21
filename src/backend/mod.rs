//! The native tools veo runs: yt-dlp, FFmpeg and FFprobe.

mod acquire;
mod media;
mod overrides;
mod report;
mod search;

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use crate::cancel::Cancel;
use crate::error::{Error, Result};
use crate::paths::{self, arch, exe_suffix, platform};
use crate::state;
use crate::tool_setup;

pub use acquire::*;
pub use media::*;
pub use overrides::*;
pub use report::*;
pub use search::*;

pub const TERMUX_SETUP: &str = "pkg install python-yt-dlp yt-dlp-ejs ffmpeg";

pub const RELEASE: &str = "2026.08.19";

pub(super) const RELEASE_HOST: &str = "https://github.com/yt-dlp/yt-dlp/releases/download";

pub(super) const MAX_BYTES: u64 = 256 * 1024 * 1024;

pub(super) const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);

// Official hashes fetched at development time; never trust a runtime checksum download.
// https://github.com/yt-dlp/yt-dlp/releases/download/2026.08.19/SHA2-256SUMS
pub(super) const HASHES: [(&str, &str); 8] = [
    ("yt-dlp.exe", "66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a"),
    ("yt-dlp_arm64.exe", "05b438997bafc3affdfda9d041353c9d73e04dc842207254b655b0887c4445b0"),
    ("yt-dlp_x86.exe", "a8f91bd41452506bc81ebd2f369b186fea0ee7075413ba00cef9fd346a0a5d0c"),
    ("yt-dlp_macos", "0f192b7ec147ab6288885d6351d9ab67367640029b4377576ef46dd79cf7b202"),
    ("yt-dlp_linux", "58162f9bfdc27458ea47bfcb311cf47028f17d8154a8bf7d689861d46399230a"),
    ("yt-dlp_linux_aarch64", "b16e4dab368a816cd05d477d698a605a6ae87ccee1c8ffd38fa21d7254141fcc"),
    ("yt-dlp_musllinux", "f3dec9cfeaf304cec98290fe41c6ad465d4b747d302473559643e7af24929722"),
    ("yt-dlp_musllinux_aarch64", "17b164c4d258be92bb1ad146cb7c336b783aedb380814aabbcb7d52937f77e57"),
];

pub(super) fn pinned_hash(asset: &str) -> Option<&'static str> {
    HASHES.iter().find(|(name, _)| *name == asset).map(|(_, hash)| *hash)
}

pub fn select_asset(platform: &str, arch: &str, musl: bool) -> Option<String> {
    match (platform, arch) {
        ("win32", "x64") => Some("yt-dlp.exe".into()),
        ("win32", "arm64") => Some("yt-dlp_arm64.exe".into()),
        ("win32", "ia32") => Some("yt-dlp_x86.exe".into()),
        ("darwin", "x64" | "arm64") => Some("yt-dlp_macos".into()),
        ("linux", "x64" | "arm64") => Some(format!("yt-dlp_{}linux{}", if musl { "musl" } else { "" }, if arch == "arm64" { "_aarch64" } else { "" })),
        _ => None,
    }
}

/// Whether this Linux system uses musl instead of glibc.
pub fn musl() -> bool {
    if platform() != "linux" {
        return false;
    }
    ["/lib", "/usr/lib"].iter().any(|directory| {
        fs::read_dir(directory).map(|entries| entries.flatten().any(|entry| entry.file_name().to_string_lossy().starts_with("ld-musl-"))).unwrap_or(false)
    })
}

pub fn cache_directory(release: &str) -> PathBuf {
    paths::cache_base().join("backends").join(release).join(format!("{}-{}", platform(), arch()))
}

pub fn release_url(release: &str, asset: &str) -> String {
    format!("{RELEASE_HOST}/{release}/{asset}")
}

pub fn is_valid_release(value: &str) -> bool {
    re!(r"^[0-9]{4}\.[0-9]{2}\.[0-9]{2}(?:\.[0-9]+)?$").is_match(value)
}

/// State file that records an explicitly installed newer backend.
pub fn backend_state_file() -> PathBuf {
    paths::cache_base().join("backend-override.json")
}

/// Resolved native tools for one run.
#[derive(Clone, Debug)]
pub struct Backend {
    pub yt_dlp: PathBuf,
    pub ffmpeg_location: PathBuf,
}

impl Backend {
    pub fn ffmpeg(&self) -> PathBuf {
        self.ffmpeg_location.join(format!("ffmpeg{}", exe_suffix()))
    }

    pub fn ffprobe(&self) -> PathBuf {
        self.ffmpeg_location.join(format!("ffprobe{}", exe_suffix()))
    }
}

/// A JavaScript runtime for yt-dlp's YouTube challenge solver, if installed.
/// VEO_JS_RUNTIME=name:path overrides the search.
pub fn js_runtime() -> Option<&'static (String, PathBuf)> {
    static RUNTIME: OnceLock<Option<(String, PathBuf)>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            if let Ok(value) = std::env::var("VEO_JS_RUNTIME") {
                let (name, path) = value.split_once(':').filter(|(name, _)| ["node", "deno", "bun", "quickjs"].contains(name))?;
                return Some((name.to_string(), PathBuf::from(path)));
            }
            ["node", "deno", "bun"].iter().find_map(|name| find(&[name]).map(|path| (name.to_string(), path)))
        })
        .as_ref()
}

/// Resolve native tools, installing missing tools on first use (unless offline).
/// VEO_YT_DLP_PATH: trusted executable file; bypasses acquisition and hashes.
/// VEO_FFMPEG_PATH: directory containing both ffmpeg and ffprobe.
pub fn resolve_backend(cancel: &Cancel, offline: bool, status: &dyn Fn(&str), status_done: &dyn Fn(&str, &str)) -> Result<Backend> {
    cancel.check()?;
    let yt_override = env_path("VEO_YT_DLP_PATH")?;
    if platform() == "android" {
        return resolve_termux(yt_override, cancel, offline, status);
    }
    let asset = if yt_override.is_some() { None } else { select_asset(platform(), arch(), musl()) };
    if yt_override.is_none() && asset.is_none() {
        return Err(Error::msg(format!("No standalone yt-dlp is available for {}/{}. Set VEO_YT_DLP_PATH to a trusted executable.", platform(), arch())));
    }
    // Validate overrides before doing any network work.
    let yt_dlp = match &yt_override {
        Some(path) => Some(executable(path, "VEO_YT_DLP_PATH")?),
        None => None,
    };
    let installed = if yt_dlp.is_some() { None } else { active_override(asset.as_deref(), cancel, &backend_state_file())? };
    let directory = cache_directory(RELEASE);
    fs::create_dir_all(&directory)?;
    let _ = state::set_mode(&directory, 0o700);
    // Hash yt-dlp while the media tools are verified on other threads.
    let pinned = asset.as_deref().and_then(pinned_hash);
    let yt_path = directory.join(format!("yt-dlp{}", exe_suffix()));
    let (media, yt_ready) = std::thread::scope(|scope| {
        let check = scope.spawn(|| match (&yt_dlp, &installed, pinned) {
            (None, None, Some(expected)) => matches(&yt_path, expected, cancel).unwrap_or(false),
            _ => true,
        });
        let media = resolve_media_tools(&directory, offline, cancel, status, status_done, &MediaHooks::default());
        (media, check.join().unwrap_or(false))
    });
    let ffmpeg_location = media?;
    if yt_dlp.is_some() {
        status("Using VEO_YT_DLP_PATH override.");
    } else if let Some(installed) = &installed {
        status(&format!("Using the installed yt-dlp {}.", installed.release));
    }
    if offline && yt_dlp.is_none() && installed.is_none() && !yt_ready {
        return Err(Error::msg("yt-dlp is missing or damaged. Run veo doctor fix without --offline to download it."));
    }
    let backend = match (yt_dlp, installed) {
        (Some(path), _) => path,
        (None, Some(installed)) => installed.path,
        (None, None) if yt_ready => {
            status("Checking yt-dlp…");
            status_done("Checking yt-dlp…", "done");
            if !cfg!(windows) {
                state::set_mode(&yt_path, 0o755)?;
            }
            yt_path
        }
        (None, None) => acquire(asset.as_deref().unwrap_or_default(), &directory, cancel, status, status_done)?,
    };
    cancel.check()?;
    Ok(Backend { yt_dlp: backend, ffmpeg_location })
}

pub(super) fn resolve_termux(yt_override: Option<PathBuf>, cancel: &Cancel, offline: bool, status: &dyn Fn(&str)) -> Result<Backend> {
    let media_override = env_path("VEO_FFMPEG_PATH")?;
    // Invalid explicit paths must fail before a package-manager mutation.
    if let Some(path) = &yt_override {
        executable(path, "VEO_YT_DLP_PATH")?;
    }
    if let Some(location) = &media_override {
        executable(&location.join("ffmpeg"), "VEO_FFMPEG_PATH ffmpeg")?;
        executable(&location.join("ffprobe"), "VEO_FFMPEG_PATH ffprobe")?;
    }
    let finder = |names: &[&str]| find(names);
    let discover = || -> Result<(Option<PathBuf>, Option<PathBuf>, Option<PathBuf>, bool)> {
        Ok((
            yt_override.clone().or_else(|| find(&["yt-dlp"])),
            media_override.as_ref().map(|location| location.join("ffmpeg")).or_else(|| find(&["ffmpeg"])),
            media_override.as_ref().map(|location| location.join("ffprobe")).or_else(|| find(&["ffprobe"])),
            yt_override.is_some() || tool_setup::has_termux_ejs(&finder, cancel)?,
        ))
    };
    let ready = |tools: &(Option<PathBuf>, Option<PathBuf>, Option<PathBuf>, bool)| tools.0.is_some() && tools.1.is_some() && tools.2.is_some() && tools.3;
    let mut tools = discover()?;
    if !ready(&tools) {
        if offline {
            return Err(Error::msg(format!(
                "Termux tools are missing (yt-dlp, FFmpeg/FFprobe or JavaScript support). Run veo doctor fix without --offline, or: {TERMUX_SETUP}"
            )));
        }
        tool_setup::install_termux_tools(&finder, cancel, status)?;
        tools = discover()?;
        if !ready(&tools) {
            return Err(Error::msg(format!("Termux setup finished but tools are still missing. Check PATH and run: {TERMUX_SETUP}")));
        }
        for (file, flag) in [(&tools.0, "--version"), (&tools.1, "-version"), (&tools.2, "-version")] {
            if let Some(file) = file {
                tool_setup::run_setup(file, &[flag], cancel, None, &|_| {}, Duration::from_secs(15))?;
            }
        }
    }
    let (Some(yt_dlp), Some(ffmpeg), Some(ffprobe), _) = tools else { unreachable!() };
    executable(&ffmpeg, "ffmpeg")?;
    executable(&ffprobe, "ffprobe")?;
    if paths::dirname(&ffmpeg) != paths::dirname(&ffprobe) {
        return Err(Error::msg("Set VEO_FFMPEG_PATH to a directory containing both ffmpeg and ffprobe."));
    }
    status(if yt_override.is_some() { "Using VEO_YT_DLP_PATH override." } else { "Using system yt-dlp (Termux package; not pinned by veo)." });
    cancel.check()?;
    Ok(Backend { yt_dlp, ffmpeg_location: paths::dirname(&ffmpeg) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil;

    #[test]
    fn selects_release_assets() {
        assert_eq!(select_asset("win32", "x64", false).as_deref(), Some("yt-dlp.exe"));
        assert_eq!(select_asset("linux", "arm64", true).as_deref(), Some("yt-dlp_musllinux_aarch64"));
        assert_eq!(select_asset("linux", "x64", false).as_deref(), Some("yt-dlp_linux"));
        assert_eq!(select_asset("darwin", "arm64", false).as_deref(), Some("yt-dlp_macos"));
        assert_eq!(select_asset("freebsd", "x64", false), None);
        for (asset, hash) in HASHES {
            assert!(hash.len() == 64, "{asset}");
        }
    }

    #[test]
    fn validates_release_names() {
        assert!(is_valid_release("2026.08.19"));
        assert!(is_valid_release("2026.08.19.1"));
        assert!(!is_valid_release("v2026.08.19"));
    }

    #[test]
    fn finds_executables_and_honours_exclusions() {
        let root = fsutil::temp_dir(&std::env::temp_dir(), "veo-find-").unwrap();
        let (first, second) = (root.join("a"), root.join("b"));
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let name = format!("tool{}", if cfg!(windows) { ".exe" } else { "" });
        for directory in [&first, &second] {
            fs::write(directory.join(&name), b"x").unwrap();
            state::set_mode(&directory.join(&name), 0o755).unwrap();
        }
        let directories = vec![first.clone(), second.clone()];
        assert_eq!(find_on_path(&["tool"], Some(&directories), &[]), Some(first.join(&name)));
        assert_eq!(find_on_path(&["tool"], Some(&directories), &[first.join(&name)]), Some(second.join(&name)));
        let _ = fs::remove_dir_all(root);
    }
}
