use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};

use crate::cancel::Cancel;
use crate::error::{Error, Result};
use crate::fsutil::{self, identity, open_nofollow};
use crate::media_assets::{media_plan, Asset, MediaPlan, MEDIA_ASSET_BASE};
use crate::paths;
use crate::process::{self, Command};
use crate::utils::clean_text;

pub type Find<'a> = &'a dyn Fn(&[&str]) -> Option<PathBuf>;

/// Run an installer or probe with fixed arguments; never through a shell.
pub fn run_setup(
    command: &Path,
    args: &[&str],
    cancel: &Cancel,
    env: Option<Vec<(OsString, OsString)>>,
    status: &dyn Fn(&str),
    timeout: Duration,
) -> Result<String> {
    let mut child = Command::new(command).args(args.iter().copied()).timeout(timeout, "Tool installation timed out. Retry with veo doctor fix.");
    child.env = env;
    let mut output = String::new();
    let code = process::run(&child, cancel, |_, data| {
        let text = clean_text(&String::from_utf8_lossy(data));
        output.push_str(&text);
        if output.len() > 8192 {
            let cut = output.len() - 8192;
            let boundary = (cut..output.len()).find(|index| output.is_char_boundary(*index)).unwrap_or(output.len());
            output.drain(..boundary);
        }
        // Surface package-manager progress without taking over stdin or stdout.
        for line in re!(r"[\r\n]+").split(&text) {
            if !line.trim().is_empty() {
                status(&line.chars().take(500).collect::<String>());
            }
        }
        Ok(())
    })?;
    if code != Some(0) {
        return Err(Error::msg(format!(
            "{} exited with code {}: {}",
            paths::basename(command),
            code.map_or("null".into(), |code| code.to_string()),
            output.trim()
        )));
    }
    Ok(output)
}

pub fn has_termux_ejs(find: Find, cancel: &Cancel) -> Result<bool> {
    let Some(python) = find(&["python", "python3"]) else { return Ok(false) };
    match run_setup(&python, &["-B", "-c", "import yt_dlp_ejs"], cancel, None, &|_| {}, Duration::from_secs(15)) {
        Ok(_) => Ok(true),
        Err(_) => {
            cancel.check()?;
            Ok(false)
        }
    }
}

static TERMUX_INSTALL: Mutex<()> = Mutex::new(());

pub fn install_termux_tools(find: Find, cancel: &Cancel, status: &dyn Fn(&str)) -> Result<()> {
    let _guard = TERMUX_INSTALL.lock().unwrap_or_else(|poison| poison.into_inner());
    let result = (|| -> Result<()> {
        let pkg = find(&["pkg"])
            .ok_or_else(|| Error::msg("Automatic Android setup requires Termux with pkg on PATH. Install python-yt-dlp, yt-dlp-ejs and ffmpeg in Termux."))?;
        cancel.check()?;
        status("Installing yt-dlp, JavaScript support and FFmpeg with Termux pkg (first use)…");
        let confirm = if std::env::var("TERMUX_APP_PACKAGE_MANAGER").is_ok_and(|manager| manager == "pacman") { "--noconfirm" } else { "-y" };
        let mut env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        env.push(("DEBIAN_FRONTEND".into(), "noninteractive".into()));
        // pkg refreshes mirrors itself. No full system upgrade or sudo.
        run_setup(&pkg, &["install", confirm, "python-yt-dlp", "yt-dlp-ejs", "ffmpeg"], cancel, Some(env), status, Duration::from_secs(600))?;
        Ok(())
    })();
    result.map_err(|cause| {
        if cancel.is_cancelled() {
            return Error::cancelled();
        }
        Error::msg(format!(
            "Automatic Termux setup failed: {}. Retry with veo doctor fix; if the repository needs repair, run pkg update in Termux.",
            cause.message
        ))
    })
}

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const MEDIA_ORIGINS: [&str; 2] = ["https://github.com", "https://release-assets.githubusercontent.com"];

fn validate_asset(asset: &Asset) -> Result<()> {
    if !re!(r"^[A-Za-z0-9][A-Za-z0-9._-]*$").is_match(asset.name) || asset.name.contains("..") {
        return Err(Error::msg("Invalid media asset name."));
    }
    if asset.size == 0 || !re!(r"^[a-f0-9]{64}$").is_match(asset.sha256) {
        return Err(Error::msg(format!("Invalid pinned integrity metadata for {}.", asset.name)));
    }
    Ok(())
}

/// Checks byte count and SHA-256 of a stream against pinned values.
struct Integrity<'a> {
    asset: &'a Asset,
    hash: Sha256,
    size: u64,
}

impl<'a> Integrity<'a> {
    fn new(asset: &'a Asset) -> Self {
        Integrity { asset, hash: Sha256::new(), size: 0 }
    }

    fn update(&mut self, data: &[u8]) -> Result<()> {
        self.size += data.len() as u64;
        if self.size > self.asset.size {
            return Err(Error::msg(format!("Unexpected size for {}: exceeds pinned byte count.", self.asset.name)));
        }
        self.hash.update(data);
        Ok(())
    }

    fn finish(self) -> Result<()> {
        if self.size != self.asset.size {
            return Err(Error::msg(format!("Unexpected size for {}: does not match pinned byte count.", self.asset.name)));
        }
        if hex::encode(self.hash.finalize()) != self.asset.sha256 {
            return Err(Error::msg(format!("Integrity verification failed for {}: SHA-256 mismatch.", self.asset.name)));
        }
        Ok(())
    }
}

fn approved(url: &url::Url) -> Result<()> {
    let origin = url.origin().ascii_serialization();
    if !MEDIA_ORIGINS.contains(&origin.as_str()) || !url.username().is_empty() || url.password().is_some() {
        return Err(Error::msg("Media download redirect must remain on an approved HTTPS origin."));
    }
    Ok(())
}

fn media_response(address: &str, cancel: &Cancel, deadline: Instant) -> Result<ureq::http::Response<ureq::Body>> {
    let mut current = url::Url::parse(address).map_err(|_| Error::msg("Invalid media download URL."))?;
    approved(&current)?;
    for redirects in 0..=5 {
        cancel.check()?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let response = crate::http::agent(remaining.max(Duration::from_millis(1)), false).get(current.as_str()).header("Accept-Encoding", "identity").call()?;
        let status = response.status().as_u16();
        if [301, 302, 303, 307, 308].contains(&status) {
            let location = crate::http::header(&response, "location").map(str::to_string);
            let (Some(location), true) = (location, redirects < 5) else { return Err(Error::msg("Media download has an invalid or excessive redirect.")) };
            current = current.join(&location).map_err(|_| Error::msg("Media download has an invalid or excessive redirect."))?;
            approved(&current)?;
            continue;
        }
        if status != 200 {
            return Err(Error::msg(format!("Media download failed with HTTP {status}.")));
        }
        return Ok(response);
    }
    Err(Error::msg("Media download has an excessive redirect."))
}

/// Download one pinned asset, verifying byte count and SHA-256 while it streams;
/// gzip transfers are verified again after decompression.
pub fn download_media_asset(asset: &Asset, release: &str, destination: &Path, cancel: &Cancel, timeout: Duration) -> Result<()> {
    validate_asset(asset)?;
    if !re!(r"^b[0-9]+\.[0-9]+(?:\.[0-9]+)?$").is_match(release) {
        return Err(Error::msg("Invalid pinned media release."));
    }
    if let Some(gzip) = asset.gzip {
        validate_asset(gzip)?;
    }
    let deadline = Instant::now() + timeout.min(DOWNLOAD_TIMEOUT);
    let scoped = cancel.child_with_timeout(timeout.min(DOWNLOAD_TIMEOUT));
    let transfer = asset.gzip.unwrap_or(asset);
    let compressed = asset.gzip.map(|_| PathBuf::from(format!("{}.gz", destination.display())));
    let transfer_path = compressed.clone().unwrap_or_else(|| destination.to_path_buf());
    let mut created_transfer = false;
    let mut created_destination = false;
    let result = (|| -> Result<()> {
        let mut response = media_response(&format!("{MEDIA_ASSET_BASE}/{release}/{}", transfer.name), &scoped, deadline)?;
        if let Some(length) = crate::http::header(&response, "content-length") {
            if length.parse::<u64>().ok() != Some(transfer.size) {
                return Err(Error::msg(format!("Unexpected content length for {}: does not match pinned byte count.", transfer.name)));
            }
        }
        let mut file = crate::state::create_private(&transfer_path)?;
        created_transfer = true;
        let mut check = Integrity::new(transfer);
        crate::http::stream(&mut response, &scoped, |data| {
            check.update(data)?;
            file.write_all(data)?;
            Ok(())
        })?;
        check.finish()?;
        file.sync_all()?;
        drop(file);
        if let Some(compressed) = &compressed {
            let mut output = crate::state::create_private(destination)?;
            created_destination = true;
            let mut check = Integrity::new(asset);
            let mut decoder = GzDecoder::new(File::open(compressed)?);
            let mut buffer = vec![0u8; 1024 * 1024];
            loop {
                scoped.check()?;
                let read = std::io::Read::read(&mut decoder, &mut buffer)?;
                if read == 0 {
                    break;
                }
                check.update(&buffer[..read])?;
                output.write_all(&buffer[..read])?;
            }
            check.finish()?;
            output.sync_all()?;
        }
        scoped.check()
    })();
    if let Some(compressed) = &compressed {
        if created_transfer {
            let _ = fs::remove_file(compressed);
        }
    }
    if let Err(error) = result {
        if created_destination {
            let _ = fs::remove_file(destination);
        }
        if created_transfer && compressed.is_none() {
            let _ = fs::remove_file(&transfer_path);
        }
        cancel.check()?;
        if scoped.timed_out() {
            return Err(Error::timeout("Media download timed out."));
        }
        return Err(error);
    }
    Ok(())
}

/// The file is a regular, non-symlinked file with the pinned size and hash,
/// and was not swapped while it was read.
pub fn verify_media_file(file: &Path, asset: &Asset, cancel: &Cancel) -> Result<bool> {
    cancel.check()?;
    let info = fs::symlink_metadata(file)?;
    if !info.is_file() || info.file_type().is_symlink() || info.len() != asset.size {
        return Ok(false);
    }
    let handle = open_nofollow(file)?;
    let opened = handle.metadata()?;
    let before = identity(&handle);
    if !opened.is_file() || opened.len() != asset.size || before.is_none() || before != fsutil::identity_of(file) {
        return Ok(false);
    }
    let mut check = Integrity::new(asset);
    let mut reader = &handle;
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        cancel.check()?;
        let read = std::io::Read::read(&mut reader, &mut buffer)?;
        if read == 0 {
            break;
        }
        if check.update(&buffer[..read]).is_err() {
            return Ok(false);
        }
    }
    if check.finish().is_err() {
        return Ok(false);
    }
    let after = fs::symlink_metadata(file)?;
    Ok(after.is_file()
        && !after.file_type().is_symlink()
        && after.len() == opened.len()
        && after.modified().ok() == opened.modified().ok()
        && fsutil::identity_of(file) == before)
}

/// Verify ffmpeg and ffprobe of a directory in parallel.
pub fn verified_media_tools(directory: &Path, platform: &str, arch: &str, cancel: &Cancel) -> Result<bool> {
    let Some(plan) = media_plan(platform, arch) else { return Ok(false) };
    let suffix = if platform == "win32" { ".exe" } else { "" };
    let check = |name: &str, asset: &Asset| verify_media_file(&directory.join(format!("{name}{suffix}")), asset, cancel).unwrap_or(false);
    let (ffmpeg, ffprobe) = std::thread::scope(|scope| {
        let ffmpeg = scope.spawn(|| check("ffmpeg", &plan.ffmpeg));
        let ffprobe = check("ffprobe", &plan.ffprobe);
        (ffmpeg.join().unwrap_or(false), ffprobe)
    });
    cancel.check()?;
    Ok(ffmpeg && ffprobe)
}

/// Native version probes do not need uploader configuration or secrets.
pub fn media_probe_environment() -> Vec<(OsString, OsString)> {
    std::env::vars_os().filter(|(name, _)| !re!(r"(?i)^(?:SMOLUP|SMOP|SMUP)_").is_match(&name.to_string_lossy())).collect()
}

/// Download, verify and install the pinned FFmpeg/FFprobe pair into `directory`.
pub fn install_media_tools(directory: &Path, cancel: &Cancel, status: &dyn Fn(&str), platform: &str, arch: &str) -> Result<PathBuf> {
    let plan: &MediaPlan = media_plan(platform, arch).ok_or_else(|| {
        Error::msg(format!("Automatic FFmpeg setup is unavailable for {platform}/{arch}. Set VEO_FFMPEG_PATH to a directory containing ffmpeg and ffprobe."))
    })?;
    cancel.check()?;
    let setup = cancel.child_with_timeout(DOWNLOAD_TIMEOUT);
    fs::create_dir_all(directory)?;
    let temporary = fsutil::temp_dir(directory, ".media-setup-")?;
    let suffix = if platform == "win32" { ".exe" } else { "" };
    let result = (|| -> Result<PathBuf> {
        status("Downloading FFmpeg and FFprobe into the veo cache (first use)…");
        let files = [
            (temporary.join("FFmpeg.LICENSE"), &plan.license),
            (temporary.join("FFmpeg.README"), &plan.readme),
            (temporary.join(format!("ffmpeg{suffix}")), &plan.ffmpeg),
            (temporary.join(format!("ffprobe{suffix}")), &plan.ffprobe),
        ];
        // Parallel transfers: the two binaries dominate the setup time.
        let outcomes: Vec<Result<()>> = std::thread::scope(|scope| {
            let handles: Vec<_> = files
                .iter()
                .map(|(file, asset)| {
                    let setup = &setup;
                    scope.spawn(move || download_media_asset(asset, plan.release, file, setup, DOWNLOAD_TIMEOUT))
                })
                .collect();
            handles.into_iter().map(|handle| handle.join().unwrap_or_else(|_| Err(Error::msg("download thread failed")))).collect()
        });
        for outcome in outcomes {
            outcome?;
        }
        // Verify the whole pair before either downloaded program may execute.
        if !verified_media_tools(&temporary, platform, arch, &setup)? {
            return Err(Error::msg("Downloaded media tools failed pinned integrity verification."));
        }
        for (file, asset) in &files[..2] {
            if !verify_media_file(file, asset, &setup)? {
                return Err(Error::msg("Downloaded media notices failed pinned integrity verification."));
            }
        }
        for (file, _) in &files[2..] {
            let _ = crate::state::set_mode(file, 0o700);
            run_setup(file, &["-version"], &setup, Some(media_probe_environment()), &|_| {}, Duration::from_secs(15))?;
        }
        // The staging directory lives inside the cache, so publishing is a rename.
        for (file, _) in &files {
            let target = directory.join(paths::basename(file));
            crate::backend::publish(file, &target, None, &setup)?;
        }
        if !verified_media_tools(directory, platform, arch, &setup)? {
            return Err(Error::msg("Staged media tools failed pinned integrity verification."));
        }
        for (file, asset) in &files[..2] {
            if !verify_media_file(&directory.join(paths::basename(file)), asset, &setup)? {
                return Err(Error::msg("Staged media notices failed pinned integrity verification."));
            }
        }
        setup.check()?;
        Ok(directory.to_path_buf())
    })();
    let _ = fs::remove_dir_all(&temporary);
    result.map_err(|cause| {
        if cancel.user_cancelled() {
            return Error::cancelled();
        }
        Error::msg(format!("Automatic FFmpeg setup failed: {}. Retry with veo doctor fix or set VEO_FFMPEG_PATH to installed tools.", cause.message))
    })
}
