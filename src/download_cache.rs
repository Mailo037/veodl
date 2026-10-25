use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{optional, Error, Result};
use crate::state;

pub const TRANSFER_RETENTION_MS: i64 = 15 * 60 * 1000;

pub fn root() -> PathBuf {
    crate::paths::cache_base().join("downloads")
}

/// `output` keeps staging beside the destination, avoiding a cross-volume copy.
/// Other explicit paths are used as-is; the default remains the local cache.
pub fn root_for(options: &crate::options::Options, fallback: &Path) -> PathBuf {
    match options.staging_dir.as_deref() {
        Some("output") => crate::paths::resolve(&options.output).join(".veo-staging"),
        Some(path) => crate::paths::resolve(path),
        None => crate::paths::resolve(fallback),
    }
}

/// Register explicit staging roots so expiration and `flush` can find them
/// without scanning arbitrary output trees. Both sides must agree on identity.
pub fn register_root(directory: &Path, cache: &Path) -> Result<()> {
    let directory = crate::paths::resolve(directory);
    fs::create_dir_all(&directory)?;
    let info = fs::symlink_metadata(&directory)?;
    if !info.is_dir() || info.file_type().is_symlink() {
        return Err(Error::msg("Staging root must be a plain directory."));
    }
    let shown = crate::paths::display(&directory);
    let id = state::digest(&Value::String(shown.clone()));
    let marker = directory.join(".veo-staging-root.json");
    if let Some(info) = optional(fs::symlink_metadata(&marker))? {
        if !info.is_file() || info.file_type().is_symlink() {
            return Err(Error::msg("Invalid staging root marker."));
        }
        let value: Option<Value> = state::read_json(&marker)?;
        if value.as_ref().and_then(|value| value.get("id")).and_then(Value::as_str) != Some(id.as_str()) {
            return Err(Error::msg("Staging root marker belongs to a different directory."));
        }
    } else {
        state::write_json(&marker, &serde_json::json!({"version":1,"id":id}))?;
    }
    state::write_json(&cache.join("staging-roots").join(format!("{id}.json")), &serde_json::json!({"version":1,"root":shown}))
}

pub fn registered_roots(cache: &Path) -> Result<Vec<PathBuf>> {
    let registry = cache.join("staging-roots");
    let Some(info) = optional(fs::symlink_metadata(&registry))? else { return Ok(Vec::new()) };
    if !info.is_dir() || info.file_type().is_symlink() {
        return Err(Error::msg("Invalid staging root registry."));
    }
    let mut roots = Vec::new();
    for entry in fs::read_dir(registry)?.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let value: Option<Value> = state::read_json(&entry.path()).ok().flatten();
        let Some(shown) = value.as_ref().filter(|value| value["version"] == 1).and_then(|value| value["root"].as_str()) else { continue };
        let directory = PathBuf::from(shown);
        let id = state::digest(&Value::String(shown.into()));
        if !directory.is_absolute() || entry.file_name().to_string_lossy() != format!("{id}.json") {
            continue;
        }
        if !fs::symlink_metadata(&directory).is_ok_and(|info| info.is_dir() && !info.file_type().is_symlink()) {
            continue;
        }
        let marker = directory.join(".veo-staging-root.json");
        if !fs::symlink_metadata(&marker).is_ok_and(|info| info.is_file() && !info.file_type().is_symlink()) {
            continue;
        }
        let value: Option<Value> = state::read_json(&marker).ok().flatten();
        if value.is_some_and(|value| value["version"] == 1 && value["id"] == id) {
            roots.push(directory);
        }
    }
    Ok(roots)
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
