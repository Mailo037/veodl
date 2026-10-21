use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cancel::Cancel;
use crate::error::{optional, Error, Result};
use crate::fsutil::{self, hash_file, is_executable};
use crate::paths::{self, arch, exe_suffix, platform};
use crate::state;
use crate::tool_setup;

pub const TERMUX_SETUP: &str = "pkg install python-yt-dlp yt-dlp-ejs ffmpeg";
pub const RELEASE: &str = "2026.08.19";
const RELEASE_HOST: &str = "https://github.com/yt-dlp/yt-dlp/releases/download";
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);
// Official hashes fetched at development time; never trust a runtime checksum download.
// https://github.com/yt-dlp/yt-dlp/releases/download/2026.08.19/SHA2-256SUMS
const HASHES: [(&str, &str); 8] = [
    ("yt-dlp.exe", "66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a"),
    ("yt-dlp_arm64.exe", "05b438997bafc3affdfda9d041353c9d73e04dc842207254b655b0887c4445b0"),
    ("yt-dlp_x86.exe", "a8f91bd41452506bc81ebd2f369b186fea0ee7075413ba00cef9fd346a0a5d0c"),
    ("yt-dlp_macos", "0f192b7ec147ab6288885d6351d9ab67367640029b4377576ef46dd79cf7b202"),
    ("yt-dlp_linux", "58162f9bfdc27458ea47bfcb311cf47028f17d8154a8bf7d689861d46399230a"),
    ("yt-dlp_linux_aarch64", "b16e4dab368a816cd05d477d698a605a6ae87ccee1c8ffd38fa21d7254141fcc"),
    ("yt-dlp_musllinux", "f3dec9cfeaf304cec98290fe41c6ad465d4b747d302473559643e7af24929722"),
    ("yt-dlp_musllinux_aarch64", "17b164c4d258be92bb1ad146cb7c336b783aedb380814aabbcb7d52937f77e57"),
];

fn pinned_hash(asset: &str) -> Option<&'static str> {
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

fn normalized(file: &Path) -> String {
    let resolved = paths::display(paths::resolve(file));
    if cfg!(windows) {
        resolved.to_lowercase()
    } else {
        resolved
    }
}

fn canonical(path: &Path) -> Option<PathBuf> {
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

fn env_path(name: &str) -> Result<Option<PathBuf>> {
    let Some(value) = std::env::var_os(name) else { return Ok(None) };
    let text = value.to_string_lossy();
    if text.trim().is_empty() || text.contains('\0') {
        return Err(Error::msg(format!("{name} must be a non-empty filesystem path.")));
    }
    Ok(Some(paths::resolve(&value)))
}

fn executable(file: &Path, label: &str) -> Result<PathBuf> {
    if !is_executable(file) {
        return Err(Error::msg(format!("{label} is missing or not executable: {}", file.display())));
    }
    Ok(paths::resolve(file))
}

fn matches(file: &Path, expected: &str, cancel: &Cancel) -> Result<bool> {
    let Some(info) = optional(fs::symlink_metadata(file))? else { return Ok(false) };
    // Do not accept cached symlinks or special files.
    if !info.is_file() || info.len() > MAX_BYTES {
        return Ok(false);
    }
    match hash_file(file, MAX_BYTES, cancel) {
        Ok(hash) => Ok(hash == expected),
        Err(error) if error.not_found() => Ok(false),
        Err(error) => Err(error),
    }
}

/// Move a verified temporary file into place; tolerate a concurrent identical publish.
pub fn publish(temp: &Path, destination: &Path, expected: Option<&str>, cancel: &Cancel) -> Result<()> {
    cancel.check()?;
    if !cfg!(windows) {
        state::set_mode(temp, 0o755)?;
    }
    match fs::rename(temp, destination) {
        Ok(()) => Ok(()),
        Err(error) => {
            let error = Error::from(error);
            let replaceable = ["EEXIST", "EPERM", "EACCES", "ENOTEMPTY", "EISDIR"].iter().any(|code| error.is_code(code));
            // Another process may have finished the same acquisition on Windows.
            if replaceable {
                if let Some(expected) = expected {
                    if matches(destination, expected, cancel)? {
                        return Ok(());
                    }
                }
                // A directory at the destination is stale cache from an older layout.
                if fs::symlink_metadata(destination).is_ok_and(|info| info.is_dir()) {
                    let _ = fs::remove_dir_all(destination);
                    if fs::rename(temp, destination).is_ok() {
                        return Ok(());
                    }
                }
            }
            Err(error)
        }
    }
}

/// Download one yt-dlp asset and publish it only after the SHA-256 of the
/// received bytes matched `expected`.
fn download_backend(asset: &str, release: &str, expected: &str, directory: &Path, cancel: &Cancel, status: &dyn Fn(&str), replace: bool) -> Result<PathBuf> {
    let destination = directory.join(format!("yt-dlp{}", exe_suffix()));
    let temp = PathBuf::from(format!("{}.{}.tmp", destination.display(), state::uuid()));
    let scoped = cancel.child_with_timeout(DOWNLOAD_TIMEOUT);
    status(&format!("Downloading yt-dlp {release}…"));
    let result = (|| -> Result<()> {
        let mut response = crate::http::get(&format!("{RELEASE_HOST}/{release}/{asset}"), DOWNLOAD_TIMEOUT, &[])?;
        if response.status().as_u16() != 200 {
            return Err(Error::msg(format!("yt-dlp download failed: HTTP {}.", response.status().as_u16())));
        }
        if crate::http::header(&response, "content-length").and_then(|length| length.parse::<u64>().ok()).is_some_and(|length| length > MAX_BYTES) {
            return Err(Error::msg("yt-dlp download exceeds the size limit."));
        }
        let mut file = state::create_private(&temp)?;
        let mut hash = <sha2::Sha256 as sha2::Digest>::new();
        let mut bytes = 0u64;
        crate::http::stream(&mut response, &scoped, |data| {
            bytes += data.len() as u64;
            if bytes > MAX_BYTES {
                return Err(Error::msg("yt-dlp download exceeds the size limit."));
            }
            sha2::Digest::update(&mut hash, data);
            file.write_all(data)?;
            Ok(())
        })?;
        file.sync_all()?;
        drop(file);
        if hex::encode(sha2::Digest::finalize(hash)) != expected {
            return Err(Error::msg("yt-dlp SHA-256 verification failed. The download was discarded."));
        }
        if replace {
            fsutil::remove_all(&destination)?;
        }
        publish(&temp, &destination, Some(expected), &scoped)
    })();
    let _ = fs::remove_file(&temp);
    match result {
        Ok(()) => Ok(destination),
        Err(cause) => {
            cancel.check()?;
            if scoped.timed_out() || cause.kind == crate::error::Kind::Timeout {
                return Err(Error::msg("yt-dlp download timed out after 3 minutes. Please retry."));
            }
            Err(Error::msg(format!("Unable to prepare yt-dlp: {} You can set VEO_YT_DLP_PATH to a trusted local executable.", cause.message)))
        }
    }
}

fn acquire(asset: &str, directory: &Path, cancel: &Cancel, status: &dyn Fn(&str), status_done: &dyn Fn(&str, &str)) -> Result<PathBuf> {
    let destination = directory.join(format!("yt-dlp{}", exe_suffix()));
    let expected = pinned_hash(asset).ok_or_else(|| Error::msg(format!("No pinned hash for {asset}.")))?;
    status("Checking yt-dlp…");
    let cached = match matches(&destination, expected, cancel) {
        Ok(cached) => {
            status_done("Checking yt-dlp…", "done");
            cached
        }
        Err(error) => {
            if !cancel.is_cancelled() {
                status_done("Checking yt-dlp…", "failed");
            }
            return Err(error);
        }
    };
    if cached {
        if !cfg!(windows) {
            state::set_mode(&destination, 0o755)?;
        }
        return Ok(destination);
    }
    status(&format!("Downloading yt-dlp {RELEASE} (first use)…"));
    download_backend(asset, RELEASE, expected, directory, cancel, status, false)
}

/// Copy a trusted system tool into the cache, verified by hash.
fn stage(source: &Path, destination: &Path, cancel: &Cancel) -> Result<()> {
    let expected = hash_file(source, MAX_BYTES, cancel)?;
    if matches(destination, &expected, cancel)? {
        if !cfg!(windows) {
            state::set_mode(destination, 0o755)?;
        }
        return Ok(());
    }
    let temp = PathBuf::from(format!("{}.{}.tmp", destination.display(), state::uuid()));
    let result = (|| -> Result<()> {
        cancel.check()?;
        crate::utils::copy_exclusive(source, &temp)?;
        if !matches(&temp, &expected, cancel)? {
            return Err(Error::msg(format!("Backend copy verification failed: {}", source.display())));
        }
        publish(&temp, destination, Some(&expected), cancel)
    })();
    let _ = fs::remove_file(&temp);
    result
}

fn managed_media_tools(directory: &Path, cancel: &Cancel) -> Result<Option<&'static str>> {
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

fn default_find(names: &[&str], exclude: &[PathBuf]) -> Option<PathBuf> {
    find_on_path(names, None, exclude)
}

fn default_install(directory: &Path, cancel: &Cancel, status: &dyn Fn(&str)) -> Result<PathBuf> {
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

fn resolve_termux(yt_override: Option<PathBuf>, cancel: &Cancel, offline: bool, status: &dyn Fn(&str)) -> Result<Backend> {
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
