use serde_json::{Map, Value};

use crate::cancel::Cancel;
use crate::config::{apply_profile, profile_names, selected_profile_name};
use crate::downloader::{default_backend, fetch_metadata, run_backend};
use crate::error::{Error, Result};
use crate::options::Options;
use crate::output::{format_output, Out};
use crate::playlist::{describe_estimate, selected_entries, size_estimate, validate_items};
use crate::progress::NullReporter;
use crate::source_discovery::{default_capture, discover_sources, format_source, parse_source_timeout, should_offer_source_discovery};
use crate::terminal::{self, Std};
use crate::utils::{available_heights, clean_text, validate_url};

fn question(prompt: &str) -> Result<String> {
    terminal::ask(&format_output(prompt, Std::Err.is_tty(), terminal::color_enabled()), Std::Err)
}

fn choose(output: &Out, prompt: &str, allowed: &[String], fallback: &str) -> Result<String> {
    loop {
        let answer = question(prompt)?;
        let answer = if answer.trim().is_empty() { fallback.to_string() } else { answer.trim().to_string() };
        if allowed.contains(&answer) {
            return Ok(answer);
        }
        output.line(format!("Choose: {}", allowed.join(", ")));
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

fn flag(defaults: &Map<String, Value>, key: &str) -> Option<bool> {
    defaults.get(key).and_then(Value::as_bool)
}

/// Ask for the arguments of one download in a terminal; None when declined.
pub fn interactive_args(config: &Map<String, Value>, cancel: &Cancel) -> Result<Option<Vec<String>>> {
    let output = Out::stderr(false);
    let mut args: Vec<String> = Vec::new();
    let profiles = profile_names(config);
    let mut defaults = apply_profile(config, None)?;
    if !profiles.is_empty() {
        let selected = selected_profile_name(config, None);
        let mut choices = profiles.clone();
        if selected.is_none() {
            choices.push("none".into());
        }
        let fallback = selected.unwrap_or_else(|| "none".into());
        let profile = choose(&output, &format!("Profile ({}) [{fallback}]: ", choices.join(", ")), &choices, &fallback)?;
        if profile != "none" {
            args.extend(["--profile".into(), profile.clone()]);
            defaults = apply_profile(config, Some(&profile))?;
        }
    }
    let url = validate_url(question("Video or playlist URL: ")?.trim())?;
    args.push(url.clone());
    if flag(&defaults, "listSources") == Some(true) {
        output.line("Listing page sources…");
        args.push("--list-sources".into());
        return Ok(Some(args));
    }
    let audio_default = if flag(&defaults, "audio") == Some(true) { "audio" } else { "video" };
    let kind = choose(&output, &format!("Download (video/audio) [{audio_default}]: "), &strings(&["video", "audio"]), audio_default)?;
    args.push(if kind == "audio" { "--audio" } else { "--no-audio" }.into());
    output.line("Reading available media…");
    // Inspect the collection first so ordinary video links need no playlist prompt.
    let mut inspection = crate::cli::parse_cli(std::slice::from_ref(&url), &defaults)?;
    inspection.playlist = true;
    let backend = default_backend(cancel, &NullReporter)?;
    let metadata = match fetch_metadata(&inspection, &backend, &run_backend, cancel, &NullReporter) {
        Ok(metadata) if metadata.formats.as_ref().is_some_and(Vec::is_empty) && !metadata.has_entries() && metadata.url.is_none() => {
            Err(Error::msg("No downloadable video formats found."))
        }
        other => other,
    };
    let metadata = match metadata {
        Ok(metadata) => metadata,
        Err(original) => {
            if !should_offer_source_discovery(&original) || flag(&defaults, "autoListSources") == Some(false) {
                return Err(original);
            }
            let single = Options { playlist: false, ..inspection.clone() };
            let inspect = |media: &str, scan: &Cancel| {
                fetch_metadata(&Options { media_url: Some(media.to_string()), ..single.clone() }, &backend, &run_backend, scan, &NullReporter)
            };
            let timeout = defaults.get("timeout").and_then(Value::as_str).map(parse_source_timeout).transpose()?;
            let found = discover_sources(&url, cancel, flag(&defaults, "deepScan") == Some(true), timeout, &inspect, &default_capture)
                .map(|found| found.sources)
                .unwrap_or_default();
            if found.is_empty() {
                return Err(original);
            }
            let selection = if found.len() == 1 {
                output.line(format!("Found video source: {}", format_source(&found[0])));
                found[0].clone()
            } else {
                for source in &found {
                    output.line(format_source(source));
                }
                loop {
                    let answer = question("Source number: ")?;
                    match answer.trim().parse::<usize>() {
                        Ok(index) if index >= 1 && index <= found.len() && !answer.trim().starts_with('0') => break found[index - 1].clone(),
                        _ => output.line(format!("Choose a number from 1 to {}.", found.len())),
                    }
                }
            };
            args.extend(["--source".into(), selection.index.to_string()]);
            inspect(&selection.url, cancel)?
        }
    };
    let collection = if metadata.is_collection() {
        let fallback = if flag(&defaults, "playlist") == Some(false) { "n" } else { "y" };
        choose(&output, &format!("Download the playlist? (y/n) [{fallback}]: "), &strings(&["y", "n"]), fallback)?
    } else {
        "n".into()
    };
    args.push(if collection == "y" { "--playlist" } else { "--no-playlist" }.into());
    output.line(clean_text(metadata.title.as_deref().filter(|title| !title.is_empty()).or(metadata.id.as_deref()).unwrap_or("Media")));
    if collection == "y" && metadata.entries.is_some() {
        for item in selected_entries(&metadata, None)? {
            output.line(format!("{}. {}", item.index, clean_text(item.entry.and_then(|entry| entry.title.as_deref()).unwrap_or("Unavailable entry"))));
        }
        let preferred = defaults.get("playlistItems").and_then(Value::as_str).unwrap_or("").to_string();
        let selection = loop {
            let answer = question(&format!("Entries (e.g. 1,3-5; Enter = {}): ", if preferred.is_empty() { "all" } else { &preferred }))?;
            let selection = if answer.trim().is_empty() { preferred.clone() } else { answer.trim().to_string() };
            let check = || -> Result<()> {
                if !selection.is_empty() {
                    validate_items(&selection)?;
                }
                selected_entries(&metadata, (!selection.is_empty()).then_some(selection.as_str()))?;
                Ok(())
            };
            match check() {
                Ok(()) => break selection,
                Err(error) => output.line(error.message),
            }
        };
        if !selection.is_empty() {
            args.extend(["--playlist-items".into(), selection.clone()]);
        }
        output.line(describe_estimate(&size_estimate(&selected_entries(&metadata, (!selection.is_empty()).then_some(selection.as_str()))?)));
    }
    if kind == "video" {
        let mut heights = available_heights(metadata.formats());
        heights.sort_unstable_by(|a, b| b.cmp(a));
        let mut choices = vec!["best".to_string()];
        choices.extend(heights.iter().map(|height| format!("{height}p")));
        if heights.is_empty() {
            output.line("Resolution metadata unavailable; using best. Set a quality cap with -q in command mode.");
        }
        let preferred =
            defaults.get("quality").and_then(Value::as_str).filter(|quality| choices.iter().any(|choice| choice == quality)).unwrap_or("best").to_string();
        let quality = choose(&output, &format!("Quality ({}) [{preferred}]: ", choices.join(", ")), &choices, &preferred)?;
        args.extend(["-q".into(), quality]);
    }
    // Switching the media type must not leave an incompatible format from a profile.
    let format = defaults.get("format").and_then(Value::as_str).unwrap_or("");
    if kind == "video" && crate::utils::AUDIO_FORMATS.contains(&format) {
        args.extend(["--format".into(), "mp4".into()]);
    }
    if kind == "audio" && crate::utils::VIDEO_FORMATS.contains(&format) {
        args.extend(["--format".into(), "mp3".into()]);
    }
    let fallback_directory = defaults
        .get("output")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| crate::paths::display(std::env::current_dir().unwrap_or_default()));
    let answer = question(&format!("Output directory [{fallback_directory}]: "))?;
    let directory = if answer.trim().is_empty() { fallback_directory } else { answer.trim().to_string() };
    args.extend(["-o".into(), directory.clone()]);
    if flag(&defaults, "incognito") == Some(true) {
        args.extend(["--no-resume".into(), "--no-skip-existing".into()]);
    } else {
        args.push("--resume".into());
        let skip_default = if flag(&defaults, "skipExisting") == Some(false) { "n" } else { "y" };
        let skip = choose(&output, &format!("Skip previously downloaded videos? (y/n) [{skip_default}]: "), &strings(&["y", "n"]), skip_default)?;
        args.push(if skip == "y" { "--skip-existing" } else { "--no-skip-existing" }.into());
    }
    output.line(format!("Ready: {kind}, {directory}"));
    if choose(&output, "Start download? (y/n) [y]: ", &strings(&["y", "n"]), "y")? == "n" {
        return Ok(None);
    }
    Ok(Some(args))
}
