use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::{json, Value};

use crate::backend::{self, RELEASE};
use crate::error::{Error, Result};
use crate::output::Out;
use crate::paths;
use crate::state;
use crate::utils::readable_error;
use crate::version::{self, CURRENT};

pub const PACKAGE: &str = "veodl";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

pub const UPDATE_HELP: &str = "veo update - keep veo current

Usage:
  veo update               Install the latest version with npm
  veo update --check       Only check whether a newer version exists
  veo up                   Short alias for veo update (also: veo up --check)
  veo upgrade              Alias for veo update
  veo check update         Alias for veo update --check

The registry can be overridden with VEO_REGISTRY (or npm_config_registry).
Set VEO_NO_UPDATE_CHECK=1 to disable the automatic post-download check.
";

pub fn default_registry() -> String {
    ["VEO_REGISTRY", "npm_config_registry"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .map(|value| value.trim().to_string())
        .find(|value| value.starts_with("http://") || value.starts_with("https://"))
        .map_or_else(|| "https://registry.npmjs.org".into(), |value| value.trim_end_matches('/').to_string())
}

/// Highest published version (from the full packument, not the "latest" tag).
pub fn fetch_latest_version(registry: &str, timeout: Duration) -> Result<String> {
    let mut response = crate::http::get(&format!("{registry}/{PACKAGE}"), timeout, &[("accept", "application/json")])?;
    if response.status().as_u16() != 200 {
        return Err(Error::msg(format!("Update check failed: HTTP {}.", response.status().as_u16())));
    }
    let data: Value = serde_json::from_str(&crate::http::text(&mut response, 64 * 1024 * 1024)?)?;
    let mut versions: Vec<String> = match data.get("versions").and_then(Value::as_object) {
        Some(versions) => versions.keys().cloned().collect(),
        None => data.get("version").and_then(Value::as_str).map(|version| vec![version.to_string()]).unwrap_or_default(),
    };
    versions.retain(|candidate| version::compare(candidate, "0").is_ok());
    versions.sort_by(|a, b| version::cmp(b, a));
    let latest = versions.into_iter().next().ok_or_else(|| Error::msg("Update check returned no versions."))?;
    if !crate::re!(r"^[0-9]+\.[0-9]+\.[0-9]+").is_match(&latest) {
        return Err(Error::msg("Update check returned an invalid version."));
    }
    Ok(latest)
}

fn state_file() -> PathBuf {
    paths::cache_base().join("update-check.json")
}

/// Throttled (once per day) post-command check. Purely informational; every
/// failure is silent so commands are never delayed or marked as failed.
pub fn maybe_update_notice(current: &str) -> Option<String> {
    if std::env::var_os("VEO_NO_UPDATE_CHECK").is_some_and(|value| !value.is_empty()) {
        return None;
    }
    let file = state_file();
    let now = state::now_ms();
    if let Some(last) = state::read_json_lenient::<Value>(&file).and_then(|state| state.get("lastCheck").and_then(Value::as_f64)) {
        if (now as f64) - last < DAY_MS as f64 {
            return None;
        }
    }
    let latest = fetch_latest_version(&default_registry(), Duration::from_secs(4)).ok();
    let message = latest
        .as_ref()
        .filter(|latest| version::cmp(latest, current).is_gt())
        .map(|latest| format!("Update available: veo {latest} (you have {current}). Run: veo up"));
    let _ = fs::create_dir_all(paths::dirname(&file));
    let _ = fs::write(&file, serde_json::to_string(&json!({ "lastCheck": now, "latest": latest })).unwrap_or_default());
    message
}

/// Remove yt-dlp caches of older pinned releases; keep the pinned one and an
/// explicitly installed newer backend.
pub fn prune_backend_caches(keep: &[&str], root: Option<&Path>) -> Result<usize> {
    let base = root.map(Path::to_path_buf).unwrap_or_else(|| paths::cache_base().join("backends"));
    let entries = match fs::read_dir(&base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) || keep.contains(&name.as_str()) {
            continue;
        }
        fs::remove_dir_all(entry.path())?;
        removed += 1;
    }
    Ok(removed)
}

/// Windows locks a running executable, so npm could not replace or remove the
/// installed veo.exe. A running file may still be renamed: move it aside to
/// the temp directory (same volume) or next to itself; the leftover copy is
/// deleted by a later run.
pub fn release_running_binary() -> Option<(PathBuf, PathBuf)> {
    if !cfg!(windows) {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    if !exe.to_string_lossy().contains("node_modules") {
        return None;
    }
    let name = format!("veo-old-{}.exe", crate::state::uuid());
    for target in [std::env::temp_dir().join(&name), exe.with_file_name(&name)] {
        if fs::rename(&exe, &target).is_ok() {
            return Some((exe, target));
        }
    }
    None
}

/// Put the binary back when npm did not replace it.
pub fn restore_binary(released: Option<(PathBuf, PathBuf)>) {
    if let Some((original, moved)) = released {
        if fs::metadata(&original).is_err() {
            let _ = fs::rename(moved, original);
        }
    }
}

/// Remove executables left behind by `release_running_binary`.
pub fn clean_released_binaries() {
    if !cfg!(windows) {
        return;
    }
    let mut directories = vec![std::env::temp_dir()];
    if let Ok(exe) = std::env::current_exe() {
        directories.push(paths::dirname(exe));
    }
    for directory in directories {
        let Ok(entries) = fs::read_dir(directory) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("veo-old-") && name.ends_with(".exe") {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// `npm <args>` without a shell; npm.cmd needs cmd.exe on Windows.
pub fn npm_command(args: &[&str]) -> Command {
    if cfg!(windows) {
        let mut command = Command::new("cmd.exe");
        command.args(["/d", "/s", "/c", &format!("npm {}", args.join(" "))]);
        command
    } else {
        let mut command = Command::new("npm");
        command.args(args);
        command
    }
}

pub fn update_main(args: &[String], stdout: &Out, stderr: &Out) -> Result<i32> {
    let mut args = crate::output::strip_color_flags(args);
    if args.first().is_some_and(|arg| arg == "up") {
        args[0] = "update".into();
    }
    let check_command = args.first().is_some_and(|arg| arg == "check");
    let rest: Vec<String> = args.iter().skip(if check_command { 2 } else { 1 }).cloned().collect();
    if rest.iter().any(|arg| arg == "-h" || arg == "--help") {
        stdout.write(UPDATE_HELP);
        return Ok(0);
    }
    let check_only = check_command || rest.iter().any(|arg| arg == "--check") || (rest.len() == 1 && rest[0] == "check");
    let unknown: Vec<&String> = rest.iter().filter(|token| *token != "--check" && !(check_only && *token == "check")).collect();
    if !unknown.is_empty() {
        return Err(Error::msg(format!(
            "Unknown option for veo update: {}. Use: veo update [--check]",
            unknown.iter().map(|value| value.as_str()).collect::<Vec<_>>().join(" ")
        )));
    }
    let latest = match fetch_latest_version(&default_registry(), Duration::from_secs(10)) {
        Ok(latest) => latest,
        Err(error) => {
            stderr.write(&format!("veo: {}\nUpdate manually with: npm install -g {PACKAGE}@latest\n", readable_error(&error)));
            return Ok(1);
        }
    };
    let newer = version::cmp(&latest, CURRENT).is_gt();
    if check_only {
        stdout.line(if newer { format!("Update available: veo {latest} (you have {CURRENT}). Run: veo up") } else { format!("veo {CURRENT} is up to date.") });
        return Ok(0);
    }
    if newer {
        stdout.line(format!("Updating veo {CURRENT} → {latest} with npm…"));
        let package = format!("{PACKAGE}@latest");
        let released = release_running_binary();
        let status = npm_command(&["install", "-g", "--no-fund", "--no-audit", &package]).status();
        if !status.as_ref().is_ok_and(|status| status.success()) {
            restore_binary(released);
        }
        match status {
            Err(error) => {
                let reason = if error.kind() == std::io::ErrorKind::NotFound {
                    "npm was not found. Install Node.js/npm, or update manually.".to_string()
                } else {
                    format!("Could not run npm: {}", readable_error(&error.into()))
                };
                stderr.write(&format!("veo: {reason}\nManual command: npm install -g {PACKAGE}@latest\n"));
                return Ok(1);
            }
            Ok(status) if !status.success() => {
                stderr.write(&format!(
                    "veo: npm exited with code {}. Update manually with: npm install -g {PACKAGE}@latest\n",
                    status.code().map_or("null".into(), |code| code.to_string())
                ));
                return Ok(1);
            }
            Ok(_) => stdout.line(format!("veo updated to {latest}. The next veo call uses the new version.")),
        }
    } else {
        stdout.line(format!("veo {CURRENT} is up to date."));
    }
    // An explicitly installed backend release must survive the pruning.
    let installed = backend::read_backend_override(&backend::backend_state_file()).and_then(|state| state.release);
    let mut keep = vec![RELEASE];
    if let Some(installed) = installed.as_deref() {
        keep.push(installed);
    }
    if let Ok(removed) = prune_backend_caches(&keep, None) {
        if removed > 0 {
            stdout.line(format!("Removed {removed} old backend cache{}.", if removed == 1 { "" } else { "s" }));
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prunes_old_backend_releases() {
        let root = crate::fsutil::temp_dir(&std::env::temp_dir(), "veo-prune-").unwrap();
        for release in ["2025.01.01", RELEASE, "2026.09.01"] {
            fs::create_dir_all(root.join(release)).unwrap();
        }
        assert_eq!(prune_backend_caches(&[RELEASE, "2026.09.01"], Some(&root)).unwrap(), 1);
        assert!(root.join(RELEASE).exists() && !root.join("2025.01.01").exists());
        let _ = fs::remove_dir_all(root);
    }
}
