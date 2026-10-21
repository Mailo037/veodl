//! Command-line arguments passed to yt-dlp.

use std::path::Path;

use crate::backend::{self, Backend};
use crate::options::Options;
use crate::paths;

use super::*;

pub(super) fn s(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

pub(super) fn backend_args(options: &Options, backend: &Backend) -> Vec<String> {
    let mut args =
        s(&["--ignore-config", "--no-plugin-dirs", "--no-colors", "--no-warnings", "--socket-timeout", "30", "--retries", "3", "--fragment-retries", "3"]);
    if let Some((name, path)) = backend::js_runtime() {
        args.extend(["--no-js-runtimes".to_string(), "--js-runtimes".to_string(), format!("{name}:{}", path.display())]);
    }
    args.extend(["--ffmpeg-location".to_string(), paths::display(&backend.ffmpeg_location)]);
    if options.incognito {
        args.push("--no-cache-dir".into());
    }
    if !options.playlist {
        args.push("--no-playlist".into());
    }
    if let Some(index) = options.entry_index {
        args.extend(["--playlist-items".into(), index.to_string()]);
    }
    args.extend(["--concurrent-fragments".into(), options.concurrent_fragments.to_string()]);
    // Credentials are opt-in per invocation and only forwarded to the backend.
    if let Some(cookies) = &options.cookies {
        args.extend(["--cookies".into(), cookies.clone()]);
    }
    if let Some(browser) = &options.cookies_from_browser {
        args.extend(["--cookies-from-browser".into(), browser.clone()]);
    }
    if !options.url.is_empty() {
        if options.media_url.is_some() {
            args.extend(["--referer".into(), options.url.clone()]);
        } else if let Ok(url) = url::Url::parse(&options.url) {
            args.extend(["--referer".into(), format!("{}/", url.origin().ascii_serialization())]);
        }
    }
    args
}

pub(super) fn media_args(options: &Options, quality: &QualityChoice) -> Vec<String> {
    let mut args = vec![
        "--no-overwrites".to_string(),
        if options.resume { "--continue" } else { "--no-continue" }.into(),
        "--newline".into(),
        "--progress".into(),
        "--progress-delta".into(),
        "0.2".into(),
    ];
    args.extend(s(&[
        "--progress-template",
        r#"download:veo-progress:{"progress":%(progress)j,"video":%(info.vcodec|null)j,"audio":%(info.acodec|null)j}"#,
        "--progress-template",
        "postprocess:veo-postprocess:%(progress)j",
        "--print",
        "after_move:veo-file:%(filepath)j",
        "--no-simulate",
    ]));
    if options.audio {
        args.extend(s(&["-f", "ba/b", "--extract-audio", "--audio-format", options.format.as_deref().unwrap_or("mp3"), "--audio-quality", "0"]));
    } else {
        if let Some(format) = &quality.format {
            args.extend(["-f".into(), format.clone()]);
        }
        // Merge into a permissive container first; e.g. H.264 cannot be merged
        // straight into WebM before the requested codec conversion runs.
        if options.compatible {
            args.extend(s(&["--merge-output-format", "mkv"]));
        } else if let Some(format) = &options.format {
            args.extend(s(&["--merge-output-format", "mkv", if options.recode { "--recode-video" } else { "--remux-video" }, format]));
        } else {
            args.extend(s(&["--merge-output-format", "mp4/mkv"]));
        }
    }
    let sort = ["vcodec:h264,acodec:aac".to_string()].into_iter().chain(quality.sort.clone()).collect::<Vec<_>>().join(",");
    // Prefer broadly playable sources; explicit recoding and WebM keep their own choice.
    let selected = if options.recode || options.format.as_deref() == Some("webm") { quality.sort.clone() } else { Some(sort) };
    if let (false, Some(selected)) = (options.audio, selected) {
        args.extend(["-S".into(), selected]);
    }
    if options.subs || options.auto_subs || options.sub_langs.is_some() || options.embed_subs {
        if options.subs || (options.embed_subs && !options.auto_subs) {
            args.push("--write-subs".into());
        }
        if options.auto_subs {
            args.push("--write-auto-subs".into());
        }
        args.extend(["--sub-langs".into(), options.sub_langs.clone().filter(|langs| !langs.is_empty()).unwrap_or_else(|| "en.*,en".into())]);
    }
    if let Some(format) = options.sub_format.as_ref().filter(|format| !format.is_empty()) {
        args.extend(["--sub-format".into(), format.clone()]);
    }
    if options.embed_subs {
        args.push("--embed-subs".into());
    }
    if options.embed_metadata {
        args.push("--embed-metadata".into());
    }
    if options.embed_thumbnail {
        args.push("--embed-thumbnail".into());
    }
    if let Some(categories) = options.sponsorblock_remove.as_ref().filter(|value| !value.is_empty()) {
        args.extend(["--sponsorblock-remove".into(), categories.clone()]);
    }
    if let Some(section) = options.section.as_ref().filter(|value| !value.is_empty()) {
        args.extend(["--download-sections".into(), section.clone(), "--force-keyframes-at-cuts".into()]);
    }
    args
}

pub fn staging_template(directory: &Path, playlist: bool) -> String {
    let name = if playlist { "media-%(playlist_index)03d.%(ext)s" } else { "media.%(ext)s" };
    paths::display(directory.join(name)).replace('%', "%%").replacen("%%(playlist_index)03d", "%(playlist_index)03d", 1).replacen("%%(ext)s", "%(ext)s", 1)
}
