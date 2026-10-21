use std::collections::HashMap;
use std::time::Duration;

use serde_json::Value;

use crate::backend::{
    self, active_override, backend_state_file, clear_backend_override, install_backend, is_valid_release, read_backend_override, release_url, select_asset,
    write_backend_override, OverrideState, RELEASE,
};
use crate::cancel::Cancel;
use crate::error::{Error, Result};
use crate::output::Out;
use crate::paths::{self, arch, platform};
use crate::utils::{clean_text, readable_error};

const API_LATEST: &str = "https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest";
const CHECKSUMS: &str = "SHA2-256SUMS";

pub const BACKEND_HELP: &str = "veo backend - manage the yt-dlp backend

Usage:
  veo backend update          Install the newest yt-dlp release
  veo backend update --check  Only report whether a newer release exists
  veo backend reset           Forget the installed release and use the pinned one
  veo backend reset --keep-files
                              Remove the pointer but keep the downloaded files
  veo backend --help          Show this help

veo ships a pinned, hash-verified yt-dlp release. `veo backend update` is an
opt-in escape hatch for when a website changed and the pinned release is too old:
it installs the newest official release after verifying its SHA-256 against that
release's own SHA2-256SUMS file over HTTPS.

That checksum does not ship with veo, so this weakens the pinned-hash guarantee:
it then rests on HTTPS and GitHub alone. `veo backend reset` returns to the
release veo was built and tested against. VEO_YT_DLP_PATH always wins over both.
";

pub fn fetch_latest_backend_release() -> Result<String> {
    let mut response = crate::http::get(API_LATEST, Duration::from_secs(10), &[("accept", "application/vnd.github+json"), ("user-agent", "veo")])?;
    if response.status().as_u16() != 200 {
        return Err(Error::msg(format!("Could not query the latest yt-dlp release: HTTP {}.", response.status().as_u16())));
    }
    let data: Value = serde_json::from_str(&crate::http::text(&mut response, 16 * 1024 * 1024)?)?;
    let tag = data.get("tag_name").and_then(Value::as_str).unwrap_or("");
    if !is_valid_release(tag) {
        return Err(Error::msg(format!(
            "The release host returned an unexpected version: {}",
            clean_text(&data.get("tag_name").map_or("undefined".into(), Value::to_string))
        )));
    }
    Ok(tag.to_string())
}

/// Parse an official SHA2-256SUMS file into a name -> hash map.
pub fn parse_checksums(text: &str) -> Result<HashMap<String, String>> {
    let mut hashes = HashMap::new();
    for line in text.split('\n') {
        if let Some(captures) = crate::re!(r"(?i)^([a-f0-9]{64})\s+\*?(.+?)\s*$").captures(line.trim()) {
            hashes.insert(paths::basename(&captures[2]), captures[1].to_lowercase());
        }
    }
    if hashes.is_empty() {
        return Err(Error::msg("The release checksum list could not be parsed."));
    }
    Ok(hashes)
}

pub fn fetch_release_checksum(release: &str, asset: &str) -> Result<String> {
    if !is_valid_release(release) {
        return Err(Error::msg(format!("Invalid yt-dlp release: {release}")));
    }
    let mut response = crate::http::get(&release_url(release, CHECKSUMS), Duration::from_secs(15), &[])?;
    if response.status().as_u16() != 200 {
        return Err(Error::msg(format!("Could not download {CHECKSUMS} for yt-dlp {release}: HTTP {}.", response.status().as_u16())));
    }
    parse_checksums(&crate::http::text(&mut response, 1024 * 1024)?)?
        .remove(asset)
        .ok_or_else(|| Error::msg(format!("yt-dlp {release} does not publish a checksum for {asset}.")))
}

pub fn backend_update_main(args: &[String], stdout: &Out, stderr: &Out) -> Result<i32> {
    let args = crate::output::strip_color_flags(args);
    let command = args.first().map(String::as_str);
    let rest = &args[args.len().min(1)..];
    if matches!(command, None | Some("-h" | "--help")) || rest.iter().any(|arg| arg == "-h" || arg == "--help") {
        stdout.write(BACKEND_HELP);
        return Ok(0);
    }
    let state_file = backend_state_file();
    let android = platform() == "android";
    if command == Some("reset") {
        let unknown: Vec<&String> = rest.iter().filter(|token| *token != "--keep-files").collect();
        if !unknown.is_empty() {
            return Err(Error::msg(format!(
                "Unknown option for veo backend reset: {}. Use: veo backend reset [--keep-files]",
                unknown.iter().map(|value| value.as_str()).collect::<Vec<_>>().join(" ")
            )));
        }
        if android {
            stdout.line("Android/Termux uses system yt-dlp or VEO_YT_DLP_PATH. There is no managed backend to reset.");
            return Ok(0);
        }
        let state = read_backend_override(&state_file);
        if !clear_backend_override(&state_file)? {
            stdout.line(format!("No installed backend to remove. veo uses the pinned release {RELEASE}."));
            return Ok(0);
        }
        if let Some(release) = state.as_ref().and_then(|state| state.release.as_deref()) {
            if !rest.iter().any(|arg| arg == "--keep-files") {
                let _ = std::fs::remove_dir_all(backend::cache_directory(release));
            }
        }
        let removed = state.and_then(|state| state.release).map_or(" the installed backend".to_string(), |release| format!(" yt-dlp {release}"));
        stdout.line(format!("Removed{removed}. veo uses the pinned release {RELEASE} again."));
        return Ok(0);
    }
    if command != Some("update") {
        return Err(Error::msg(format!(
            "Unknown backend command: {}. Use: veo backend update [--check] | veo backend reset",
            clean_text(command.unwrap_or(""))
        )));
    }
    let check_only = rest.iter().any(|arg| arg == "--check");
    let unknown: Vec<&String> = rest.iter().filter(|token| *token != "--check").collect();
    if !unknown.is_empty() {
        return Err(Error::msg(format!(
            "Unknown option for veo backend update: {}. Use: veo backend update [--check]",
            unknown.iter().map(|value| value.as_str()).collect::<Vec<_>>().join(" ")
        )));
    }
    if android {
        stdout.line(
            "Android/Termux uses system yt-dlp or VEO_YT_DLP_PATH, not a pinned release. Update the Termux package with: pkg upgrade python-yt-dlp yt-dlp-ejs",
        );
        return Ok(0);
    }
    if std::env::var_os("VEO_YT_DLP_PATH").is_some() {
        stderr.write("veo: VEO_YT_DLP_PATH is set and always takes precedence over the managed backend.\n");
    }
    let cancel = Cancel::global();
    let asset = select_asset(platform(), arch(), backend::musl());
    let info = (|| -> Result<(Option<String>, String)> {
        let active = active_override(asset.as_deref(), cancel, &state_file)?;
        Ok((active.map(|active| active.release), fetch_latest_backend_release()?))
    })();
    let (active, latest) = match info {
        Ok(info) => info,
        Err(error) => {
            stderr.write(&format!("veo: {}\n", readable_error(&error)));
            return Ok(1);
        }
    };
    let current = active.unwrap_or_else(|| RELEASE.into());
    let Some(asset) = asset else {
        stderr.write(&format!("veo: no standalone yt-dlp is published for {}/{}. Set VEO_YT_DLP_PATH to a trusted executable.\n", platform(), arch()));
        return Ok(1);
    };
    if !crate::version::cmp(&latest, &current).is_gt() {
        stdout.line(format!(
            "yt-dlp {current} is up to date{}.",
            if current == RELEASE { " (pinned release, hash verified at build time)" } else { " (installed release)" }
        ));
        return Ok(0);
    }
    if check_only {
        stdout.line(format!("Newer yt-dlp release available: {latest} (using {current}). Run: veo backend update"));
        return Ok(0);
    }
    let sha256 = match fetch_release_checksum(&latest, &asset) {
        Ok(hash) => hash,
        Err(error) => {
            stderr.write(&format!("veo: {}\nNothing was installed. The pinned release {RELEASE} is still in use.\n", readable_error(&error)));
            return Ok(1);
        }
    };
    stdout.line(format!("Installing yt-dlp {latest} for {}/{asset}…", platform()));
    if let Err(error) = install_backend(&latest, Some(&asset), &sha256, cancel, &|message| stdout.line(message)) {
        stderr.write(&format!("veo: {}\nThe pinned release {RELEASE} is still in use.\n", readable_error(&error)));
        return Ok(1);
    }
    let record = OverrideState {
        release: Some(latest.clone()),
        asset: Some(asset.clone()),
        sha256: Some(sha256),
        platform: Some(platform().into()),
        arch: Some(arch().into()),
        installed: Some(crate::state::iso_now()),
    };
    if let Err(error) = write_backend_override(&record, &state_file) {
        stderr.write(&format!("veo: {}\nThe download succeeded, but veo could not record it; the pinned release stays in use.\n", readable_error(&error)));
        return Ok(1);
    }
    stdout.line(format!("yt-dlp {latest} installed and will be used for the next download."));
    stderr.write(&format!("veo: note: yt-dlp {latest} was verified against the checksum published with that release over HTTPS, not against a hash shipped with veo. Run \"veo backend reset\" to return to the pinned release.\n"));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_checksums() {
        let hash = "a".repeat(64);
        let hashes = parse_checksums(&format!("{hash}  yt-dlp.exe\n{hash} *dir/yt-dlp_linux\n")).unwrap();
        assert_eq!(hashes["yt-dlp.exe"], hash);
        assert_eq!(hashes["yt-dlp_linux"], hash);
        assert!(parse_checksums("nothing").is_err());
    }
}
