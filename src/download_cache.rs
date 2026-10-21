use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{optional, Result};
use crate::state;

pub const TRANSFER_RETENTION_MS: i64 = 15 * 60 * 1000;

pub fn root() -> PathBuf {
    crate::paths::cache_base().join("downloads")
}

/// Remove expired retained transfers. Directories locked by a running
/// download, or whose state cannot be read, are left untouched.
pub fn cleanup(root: &Path, now: i64) -> Result<()> {
    fs::create_dir_all(root)?;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !re!(r"^\.veo-part-[a-f0-9]{24}$").is_match(&name) {
            continue;
        }
        let directory = root.join(&name);
        let Some(info) = optional(fs::symlink_metadata(&directory))? else { continue };
        if !info.is_dir() || info.file_type().is_symlink() {
            continue;
        }
        let lock_path = directory.join(".lock");
        let lock = match fs::OpenOptions::new().write(true).create_new(true).open(&lock_path) {
            Ok(lock) => lock,
            Err(error) if matches!(error.kind(), std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::NotFound) => continue,
            Err(error) => return Err(error.into()),
        };
        let manifest: Option<Value> = state::read_json(&directory.join("job.json")).ok().flatten();
        let expired = manifest.is_some_and(|manifest| {
            manifest.get("version").and_then(Value::as_i64) == Some(1)
                && manifest.get("expiresAt").and_then(Value::as_f64).is_some_and(|expires| expires <= now as f64)
        });
        drop(lock);
        if expired {
            // The lock stays present until the private cache directory is removed.
            let _ = fs::remove_dir_all(&directory);
        } else {
            let _ = fs::remove_file(&lock_path);
        }
    }
    Ok(())
}
