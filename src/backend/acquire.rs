//! Hash-verified download, staging and publishing of tool binaries.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::cancel::Cancel;
use crate::error::{optional, Error, Result};
use crate::fsutil::{self, hash_file};
use crate::paths::exe_suffix;
use crate::state;

use super::*;

pub(super) fn matches(file: &Path, expected: &str, cancel: &Cancel) -> Result<bool> {
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
pub(super) fn download_backend(
    asset: &str,
    release: &str,
    expected: &str,
    directory: &Path,
    cancel: &Cancel,
    status: &dyn Fn(&str),
    replace: bool,
) -> Result<PathBuf> {
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

pub(super) fn acquire(asset: &str, directory: &Path, cancel: &Cancel, status: &dyn Fn(&str), status_done: &dyn Fn(&str, &str)) -> Result<PathBuf> {
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
pub(super) fn stage(source: &Path, destination: &Path, cancel: &Cancel) -> Result<()> {
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
