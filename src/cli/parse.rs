//! Validating arguments into download options.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::config::{apply_profile, selected_profile_name};
use crate::error::{Error, Result};
use crate::naming::validate_template;
use crate::options::Options;
use crate::playlist::validate_items;
use crate::source_discovery::parse_source_timeout;
use crate::utils::{clean_text, js_trim, validate_browser_spec, validate_cookie_file, validate_url, AUDIO_FORMATS, QUALITIES, VIDEO_FORMATS};

use super::*;

pub(super) fn text(values: &HashMap<String, Val>, name: &str) -> Option<String> {
    match values.get(name) {
        Some(Val::Str(text)) => Some(text.clone()),
        _ => None,
    }
}

pub(super) fn flag(values: &HashMap<String, Val>, name: &str) -> bool {
    matches!(values.get(name), Some(Val::Bool(true)))
}

/// Turn arguments plus config defaults into validated options.
pub fn parse_cli(args: &[String], config: &Map<String, Value>) -> Result<Options> {
    let mut args = args.to_vec();
    if args.first().is_some_and(|arg| arg == "help") {
        args[0] = "--help".into();
    }
    if args.first().is_some_and(|arg| arg == "version") {
        if args.len() != 1 {
            return Err(Error::msg("Usage: veo version"));
        }
        return Ok(Options { version: true, ..Options::default() });
    }
    let preliminary = parse_args(&args, &HashMap::new())?;
    let requested_profile = text(&preliminary.values, "profile");
    let profile_name = selected_profile_name(config, requested_profile.as_deref());
    let config = apply_profile(config, requested_profile.as_deref())?;
    let Parsed { mut values, positionals, typed } = parse_args(&args, &option_defaults(&config))?;
    if flag(&values, "help") {
        return Ok(Options { help: true, help_all: positionals.first().is_some_and(|arg| arg == "all"), ..Options::default() });
    }
    if flag(&values, "version") {
        return Ok(Options { version: true, ..Options::default() });
    }
    let set = |values: &mut HashMap<String, Val>, name: &str, value: Option<Val>| match value {
        Some(value) => {
            values.insert(name.into(), value);
        }
        None => {
            values.remove(name);
        }
    };
    // An explicit numeric --quality states video intent, so it overrides an
    // audio-only default instead of turning into a confusing conflict error.
    let quality = text(&values, "quality").unwrap_or_else(|| "best".into());
    if typed.contains("quality") && quality != "best" && flag(&values, "audio") && !typed.contains("audio") {
        set(&mut values, "audio", Some(Val::Bool(false)));
    }
    let audio = flag(&values, "audio");
    if positionals.is_empty() && text(&values, "batch-file").is_none() && text(&values, "retry-failed").is_none() {
        return Err(Error::msg("Provide at least one video URL. Run veo --help for usage."));
    }
    if !QUALITIES.contains(&quality.as_str()) && !re!(r"^[1-9][0-9]{1,4}p$").is_match(&quality) {
        return Err(Error::msg(format!("Invalid quality. Choose: {} or a numeric resolution.", QUALITIES.join(", "))));
    }
    if let Some(items) = text(&values, "playlist-items") {
        if !flag(&values, "playlist") && typed.contains("playlist") {
            if typed.contains("playlist-items") {
                return Err(Error::msg("--playlist-items cannot be combined with --no-playlist."));
            }
            set(&mut values, "playlist-items", None);
        } else {
            validate_items(&items)?;
            set(&mut values, "playlist", Some(Val::Bool(true)));
        }
    }
    let retry = text(&values, "retry-failed");
    let batch = text(&values, "batch-file");
    if retry.is_some() && (!positionals.is_empty() || batch.is_some()) {
        return Err(Error::msg("--retry-failed cannot be combined with URLs or --batch-file."));
    }
    let incognito = flag(&values, "incognito");
    if incognito && retry.is_some() {
        return Err(Error::msg("--incognito cannot retry a saved job. Use --no-incognito for this request."));
    }
    for (name, message) in
        [("resume", "--incognito cannot be combined with --resume."), ("skip-existing", "--incognito cannot be combined with --skip-existing.")]
    {
        if incognito && flag(&values, name) {
            if typed.contains(name) {
                return Err(Error::msg(message));
            }
            set(&mut values, name, Some(Val::Bool(false)));
        }
    }
    let output = text(&values, "output").unwrap_or_default();
    if js_trim(&output).is_empty() {
        return Err(Error::msg("The output directory cannot be empty."));
    }
    let rename = text(&values, "rename");
    if rename.as_ref().is_some_and(|rename| clean_text(rename).is_empty()) {
        return Err(Error::msg("The custom filename cannot be empty."));
    }
    if !flag(&values, "neutral-filename") && positionals.len() > 1 && rename.as_ref().is_some_and(|rename| !rename.contains('*')) {
        return Err(Error::msg("--rename only applies to a single URL unless the name contains * for the original title."));
    }
    if audio && quality != "best" && typed.contains("quality") {
        return Err(Error::msg("--quality is for video; omit it when using --audio."));
    }
    if audio && flag(&values, "closest-quality") && typed.contains("closest-quality") {
        return Err(Error::msg("--closest-quality is for video; omit it when using --audio."));
    }
    if flag(&values, "closest-quality") && quality == "best" && typed.contains("closest-quality") {
        return Err(Error::msg("--closest-quality requires a numeric --quality such as 1080p."));
    }
    let list_qualities = flag(&values, "list-qualities") || flag(&values, "list-qualitys");
    let single = positionals.len() == 1 && batch.is_none() && retry.is_none();
    if list_qualities {
        if !single {
            return Err(Error::msg("--list-qualities accepts exactly one URL."));
        }
        if flag(&values, "list-formats") || flag(&values, "list-sources") || flag(&values, "list-subs") {
            return Err(Error::msg("--list-qualities cannot be combined with other listing actions."));
        }
        if audio || flag(&values, "playlist") {
            return Err(Error::msg("--list-qualities requires a single video without --audio or --playlist."));
        }
    }
    let format = text(&values, "format");
    if flag(&values, "list-formats") && (audio || format.is_some()) {
        return Err(Error::msg("--list-formats cannot be combined with --audio or --format."));
    }
    if flag(&values, "list-formats") && positionals.len() > 1 {
        return Err(Error::msg("--list-formats accepts exactly one URL."));
    }
    if flag(&values, "list-subs") {
        if !single {
            return Err(Error::msg("--list-subs accepts exactly one URL."));
        }
        if flag(&values, "list-formats") || flag(&values, "list-sources") {
            return Err(Error::msg("--list-subs cannot be combined with other listing actions."));
        }
        if flag(&values, "playlist") {
            return Err(Error::msg("--list-subs requires a single video without --playlist."));
        }
    }
    if let Some(sub_format) = text(&values, "sub-format") {
        if !re!(r"(?i)^[a-z0-9]+(?:/[a-z0-9]+)*$").is_match(&sub_format) || sub_format.len() > 100 {
            return Err(Error::msg("--sub-format must be a format or preference list such as srt/vtt/best."));
        }
    }
    let source = text(&values, "source");
    if let Some(source) = &source {
        if !re!(r"^[1-9][0-9]*$").is_match(source) || source.parse::<u64>().map_or(true, |value| value > 9_007_199_254_740_991) {
            return Err(Error::msg("--source requires a positive source number."));
        }
    }
    if source.is_some() && flag(&values, "list-sources") && !typed.contains("list-sources") {
        set(&mut values, "list-sources", Some(Val::Bool(false)));
    }
    if flag(&values, "list-sources") && !typed.contains("list-sources") && !single {
        set(&mut values, "list-sources", Some(Val::Bool(false)));
    }
    if flag(&values, "list-sources") && !single {
        return Err(Error::msg("--list-sources accepts exactly one URL."));
    }
    if source.is_some() && (positionals.len() > 1 || batch.is_some()) {
        return Err(Error::msg("--source accepts exactly one URL."));
    }
    if source.is_some() && flag(&values, "list-sources") {
        return Err(Error::msg("--source and --list-sources are separate actions."));
    }
    let timeout = text(&values, "timeout");
    let timeout_ms = timeout.as_deref().map(parse_source_timeout).transpose()?;
    if (source.is_some() || flag(&values, "list-sources")) && !typed.contains("playlist") && !typed.contains("playlist-items") {
        set(&mut values, "playlist", Some(Val::Bool(false)));
        set(&mut values, "playlist-items", None);
    }
    if (source.is_some() || flag(&values, "list-sources")) && flag(&values, "playlist") {
        return Err(Error::msg("--source and --list-sources cannot be combined with --playlist."));
    }
    let fragments = parse_number(&text(&values, "concurrent-fragments").unwrap_or_else(|| "8".into()));
    crate::experimental::validate_fragments(fragments)?;
    let concurrency = parse_number(&text(&values, "playlist-concurrency").unwrap_or_default());
    if !(1.0..=4.0).contains(&concurrency) || concurrency.fract() != 0.0 {
        return Err(Error::msg("--playlist-concurrency must be a whole number between 1 and 4."));
    }
    let recode = flag(&values, "recode");
    let compatible = flag(&values, "compatible");
    if compatible
        && (audio || format.as_deref().is_some_and(|format| format != "mp4") || recode || flag(&values, "embed-subs") || flag(&values, "embed-thumbnail"))
    {
        return Err(Error::msg("--compatible requires video MP4 without --recode, --embed-subs or --embed-thumbnail."));
    }
    let format = if compatible { Some("mp4".to_string()) } else { format };
    if recode && (format.is_none() || audio) {
        return Err(Error::msg("--recode requires --format and video mode."));
    }
    let section = text(&values, "section");
    if section.as_ref().is_some_and(|section| !re!(r"^[*0-9]").is_match(js_trim(section))) {
        return Err(Error::msg("--section requires a range such as \"*10:00-12:00\" or \"10:00-12:00\"."));
    }
    let sponsor = text(&values, "sponsorblock-remove");
    if sponsor.as_ref().is_some_and(|value| !re!(r"(?i)^[a-z_,-]+$").is_match(js_trim(value))) {
        return Err(Error::msg("--sponsorblock-remove takes comma-separated category names, e.g. \"sponsor,selfpromo\"."));
    }
    let formats: &[&str] = if audio { &AUDIO_FORMATS } else { &VIDEO_FORMATS };
    if let Some(format) = &format {
        if !formats.contains(&format.as_str()) {
            let hint = if !audio && AUDIO_FORMATS.contains(&format.as_str()) { " Use --audio for audio formats." } else { "" };
            return Err(Error::msg(format!("Invalid {} format. Choose: {}.{hint}", if audio { "audio" } else { "video" }, formats.join(", "))));
        }
    }
    let concurrent_downloads = parse_number(&text(&values, "concurrent-downloads").unwrap_or_default());
    let urls: Vec<String> = positionals.iter().map(|url| validate_url(url)).collect::<Result<_>>()?;
    let mut options = Options {
        quality,
        output,
        rename,
        format,
        compatible,
        recode,
        concurrent_downloads: concurrent_downloads.max(0.0) as u32,
        filename_template: text(&values, "filename-template"),
        folder_template: text(&values, "folder-template"),
        adaptive_concurrency: flag(&values, "adaptive-concurrency"),
        check_space: flag(&values, "check-space"),
        timings: flag(&values, "timings"),
        color: flag(&values, "color"),
        playlist_concurrency: concurrency as u32,
        cookies: text(&values, "cookies").map(|file| validate_cookie_file(&file)).transpose()?,
        cookies_from_browser: text(&values, "cookies-from-browser").map(|spec| validate_browser_spec(&spec)).transpose()?,
        concurrent_fragments: fragments as u32,
        experimental_fragments: flag(&values, "experimental-fragments"),
        sub_langs: text(&values, "sub-langs"),
        sponsorblock_remove: sponsor.map(|value| js_trim(&value).to_string()),
        section: section.map(|value| js_trim(&value).to_string()),
        open: flag(&values, "open"),
        audio,
        resume: flag(&values, "resume"),
        incognito,
        neutral_filename: flag(&values, "neutral-filename"),
        playlist: flag(&values, "playlist"),
        subs: flag(&values, "subs"),
        auto_subs: flag(&values, "auto-subs"),
        sub_format: text(&values, "sub-format"),
        verify: flag(&values, "verify"),
        embed_subs: flag(&values, "embed-subs"),
        embed_metadata: flag(&values, "embed-metadata"),
        embed_thumbnail: flag(&values, "embed-thumbnail"),
        closest_quality: flag(&values, "closest-quality"),
        json: flag(&values, "json"),
        skip_existing: flag(&values, "skip-existing"),
        playlist_items: text(&values, "playlist-items"),
        source,
        timeout,
        deep_scan: flag(&values, "deep-scan"),
        list_sources: flag(&values, "list-sources"),
        auto_list_sources: flag(&values, "auto-list-sources"),
        list_qualities,
        list_formats: flag(&values, "list-formats"),
        list_subs: flag(&values, "list-subs"),
        dry_run: flag(&values, "dry-run"),
        batch_file: batch,
        retry_failed: retry,
        url: urls.first().cloned().unwrap_or_default(),
        urls,
        timeout_ms,
        profile: profile_name,
        ..Options::default()
    };
    if !(1..=4).contains(&options.concurrent_downloads) || concurrent_downloads.fract() != 0.0 {
        return Err(Error::msg("--concurrent-downloads must be a whole number between 1 and 4."));
    }
    if let Some(template) = &options.filename_template {
        validate_template(template, false)?;
    }
    if let Some(template) = &options.folder_template {
        validate_template(template, true)?;
    }
    if options.rename.is_some() && options.filename_template.is_some() && !options.neutral_filename {
        return Err(Error::msg("--rename and --filename-template cannot be combined."));
    }
    let disable_subs = !options.subs && typed.contains("subs");
    if options.sub_langs.is_some() && !disable_subs && !options.auto_subs {
        options.subs = true;
    }
    if disable_subs {
        options.embed_subs = false;
        if !options.auto_subs {
            options.sub_langs = None;
        }
    }
    Ok(options)
}

/// JavaScript `Number(text)` for option values; NaN for anything else.
pub(super) fn parse_number(text: &str) -> f64 {
    let trimmed = js_trim(text);
    if trimmed.is_empty() {
        return 0.0;
    }
    trimmed.parse::<f64>().unwrap_or(f64::NAN)
}
