use std::fs;
use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

use crate::cancel::Cancel;
use crate::error::{Error, Result};
use crate::metadata::Format;
use crate::paths;

pub const QUALITIES: [&str; 7] = ["best", "2160p", "1440p", "1080p", "720p", "480p", "360p"];
pub const VIDEO_FORMATS: [&str; 4] = ["mp4", "mkv", "webm", "mov"];
pub const AUDIO_FORMATS: [&str; 6] = ["mp3", "m4a", "aac", "opus", "flac", "wav"];
// Browsers yt-dlp can read cookies from. veo only forwards the choice; it never
// decrypts or copies cookie stores itself.
pub const COOKIE_BROWSERS: [&str; 9] = ["brave", "chrome", "chromium", "edge", "firefox", "opera", "safari", "vivaldi", "whale"];
const COOKIE_KEYRINGS: [&str; 3] = ["gnomekeyring", "kwallet", "basic"];

/// JavaScript's String#trim whitespace set.
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

pub fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}

pub fn validate_url(input: &str) -> Result<String> {
    let invalid = || Error::msg("Invalid URL. Use a complete http:// or https:// video URL without embedded credentials.");
    let url = url::Url::parse(input).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) || !url.username().is_empty() || url.password().is_some() {
        return Err(invalid());
    }
    Ok(url.as_str().to_string())
}

fn unsafe_char(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '\u{00}'..='\u{1f}' | '\u{7f}'..='\u{9f}'
        | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Untrusted site titles must never become paths, terminal escapes, or device names.
pub fn sanitize_title(title: &str) -> String {
    let source = if title.is_empty() { "video" } else { title };
    let replaced: String = source.nfc().map(|c| if unsafe_char(c) { '_' } else { c }).collect();
    let mut name = js_trim(&replaced).trim_end_matches(['.', ' ']).to_string();
    if re!(r"(?i)^(con|prn|aux|nul|com[1-9¹²³]|lpt[1-9¹²³])(?:\.|$)").is_match(&name) {
        name.insert(0, '_');
    }
    let mut shortened = String::new();
    for c in name.chars() {
        if shortened.len() + c.len_utf8() > 180 {
            break;
        }
        shortened.push(c);
    }
    let trimmed = shortened.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        "video".into()
    } else {
        trimmed.into()
    }
}

/// Remove terminal escapes and control characters from untrusted text.
pub fn clean_text(value: &str) -> String {
    let without_escapes = re!(r"\x1b\[[0-?]*[ -/]*[@-~]").replace_all(value, "");
    let spaced: String = without_escapes
        .chars()
        .map(|c| if matches!(c, '\u{00}'..='\u{1f}' | '\u{7f}'..='\u{9f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') { ' ' } else { c })
        .collect();
    js_trim(&spaced).to_string()
}

/// Locale-independent local timestamp for CLI output, e.g. "2026-02-03 14:22".
pub fn local_stamp(value: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(value) {
        Ok(time) => time.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string(),
        Err(_) => "unknown".into(),
    }
}

/// Validate a Netscape-format cookie file before any network work starts. The
/// file is only read by the backend; veo never parses or stores its contents.
pub fn validate_cookie_file(input: &str) -> Result<String> {
    let raw = js_trim(input);
    if raw.is_empty() {
        crate::bail!("--cookies requires the path to a Netscape-format cookie file.");
    }
    if raw.contains('\0') {
        crate::bail!("The cookie file path contains an invalid character.");
    }
    let file = paths::resolve(raw);
    let shown = file.display();
    let info = fs::metadata(&file).map_err(|_| crate::err!("The cookie file does not exist: {shown}"))?;
    if !info.is_file() {
        crate::bail!("The cookie file is not a regular file: {shown}");
    }
    if fs::File::open(&file).is_err() {
        crate::bail!("The cookie file is not readable: {shown}");
    }
    Ok(paths::display(&file))
}

/// yt-dlp syntax: BROWSER[+KEYRING][:PROFILE][::CONTAINER]
pub fn validate_browser_spec(input: &str) -> Result<String> {
    let value = clean_text(input);
    if value.is_empty() {
        crate::bail!("--cookies-from-browser requires a browser name.");
    }
    let limit = "--cookies-from-browser accepts at most BROWSER[:PROFILE][::CONTAINER].";
    let parts: Vec<&str> = value.split("::").collect();
    if parts.len() > 2 {
        return Err(Error::msg(limit));
    }
    let (head, container) = (parts[0], parts.get(1).copied());
    let pieces: Vec<&str> = head.split(':').collect();
    if pieces.len() > 2 {
        return Err(Error::msg(limit));
    }
    let (browser, profile) = (pieces[0], pieces.get(1).copied());
    let names: Vec<&str> = browser.split('+').collect();
    if !COOKIE_BROWSERS.contains(&names[0].to_lowercase().as_str()) {
        crate::bail!("Unsupported browser for --cookies-from-browser. Choose: {}.", COOKIE_BROWSERS.join(", "));
    }
    if names.len() > 2 {
        crate::bail!("At most one keyring may follow \"+\" in --cookies-from-browser.");
    }
    if let Some(keyring) = names.get(1).filter(|keyring| !keyring.is_empty()) {
        if !COOKIE_KEYRINGS.contains(&keyring.to_lowercase().as_str()) {
            crate::bail!("Unsupported keyring for --cookies-from-browser. Choose: {}.", COOKIE_KEYRINGS.join(", "));
        }
    }
    for (label, part) in [("profile", profile), ("container", container)] {
        let Some(part) = part else { continue };
        let trimmed = js_trim(part);
        if trimmed.is_empty() || clean_text(part) != trimmed || trimmed.starts_with('-') {
            crate::bail!("The --cookies-from-browser {label} is empty or contains invalid characters.");
        }
    }
    Ok(value)
}

/// A cookie file readable by other accounts is a credential leak worth one line.
pub fn cookie_file_warning(file: Option<&str>) -> Option<String> {
    let file = file?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(info) = fs::metadata(file) {
            if info.permissions().mode() & 0o077 != 0 {
                return Some(format!("Warning: {file} is readable by other users. Restrict it with: chmod 600 \"{file}\""));
            }
        }
    }
    let _ = file;
    None
}

/// Video heights a site actually offers, ignoring audio-only and DRM entries.
pub fn available_heights(formats: &[Format]) -> Vec<u32> {
    let mut heights = Vec::new();
    for format in formats {
        if !format.is_video() || format.drm() {
            continue;
        }
        if let Some(height) = format.height.filter(|height| height.is_finite() && *height > 0.0) {
            let height = height as u32;
            if !heights.contains(&height) {
                heights.push(height);
            }
        }
    }
    heights
}

pub fn quality_target(quality: &str) -> u32 {
    quality.trim_end_matches('p').parse().unwrap_or(0)
}

pub fn closest_height(formats: &[Format], quality: &str) -> Option<u32> {
    if quality == "best" {
        return None;
    }
    let target = i64::from(quality_target(quality));
    let mut heights = available_heights(formats);
    // Equidistant alternatives prefer the smaller download.
    heights.sort_by_key(|height| ((i64::from(*height) - target).abs(), *height));
    heights.first().copied()
}

/// The highest offered resolution at or below the request.
pub fn capped_height(formats: &[Format], quality: &str) -> Option<u32> {
    let target = quality_target(quality);
    available_heights(formats).into_iter().filter(|height| *height <= target).max()
}

// Filesystems without hard links (FAT/exFAT, some SMB and container mounts)
// report one of these; anything else is a real error and must surface.
const NO_HARDLINK: [&str; 8] = ["EXDEV", "EPERM", "EACCES", "ENOTSUP", "EOPNOTSUPP", "ENOSYS", "EMLINK", "EINVAL"];

/// Occupy `destination` without ever replacing an existing file. Returns false
/// when the name is taken. A hard link is an O(1) metadata operation when
/// supported; cross-drive saves and filesystems that cannot link fall back to
/// copying. Both paths are race-safe against concurrent veo processes.
pub fn allocate(source: &Path, destination: &Path) -> Result<bool> {
    match fs::hard_link(source, destination) {
        Ok(()) => return Ok(true),
        Err(error) => {
            let error = Error::from(error);
            if error.is_code("EEXIST") {
                return Ok(false);
            }
            if error.is_code("EISDIR") {
                // Some virtual drives report EISDIR for unsupported hard links even when
                // the source is a regular file. Do not mistake a real directory for media.
                if !fs::symlink_metadata(source)?.is_file() {
                    return Err(error);
                }
                if crate::error::optional(fs::symlink_metadata(destination))?.is_some() {
                    return Ok(false);
                }
            } else if !NO_HARDLINK.iter().any(|code| error.is_code(code)) {
                return Err(error);
            }
        }
    }
    match copy_exclusive(source, destination) {
        Ok(()) => {}
        Err(error) if error.is_code("EEXIST") => return Ok(false),
        Err(error) => return Err(error),
    }
    let (source_info, destination_info) = (fs::symlink_metadata(source)?, fs::symlink_metadata(destination)?);
    if source_info.len() != destination_info.len() {
        let _ = fs::remove_file(destination);
        crate::bail!("The saved copy has an unexpected size. The local original has been preserved.");
    }
    Ok(true)
}

/// Copy into a file that must not exist yet (COPYFILE_EXCL).
pub fn copy_exclusive(source: &Path, destination: &Path) -> Result<()> {
    let mut input = fs::File::open(source)?;
    let mut output = fs::OpenOptions::new().write(true).create_new(true).open(destination)?;
    let result = std::io::copy(&mut input, &mut output).and_then(|_| output.sync_all());
    if let Err(error) = result {
        drop(output);
        let _ = fs::remove_file(destination);
        return Err(error.into());
    }
    if let Ok(info) = input.metadata() {
        let _ = output.set_permissions(info.permissions());
    }
    Ok(())
}

pub fn save_unique(source: &Path, directory: &Path, title: &str, cancel: &Cancel, keep_source: bool) -> Result<PathBuf> {
    let extension = paths::extname(source).to_lowercase();
    if !re!(r"^\.[a-z0-9]{1,8}$").is_match(&extension) {
        crate::bail!("The backend returned an invalid output extension.");
    }
    let name = sanitize_title(title);
    for number in 0u64.. {
        cancel.check()?;
        let suffix = if number > 0 { format!(" ({number})") } else { String::new() };
        let destination = directory.join(format!("{name}{suffix}{extension}"));
        if !allocate(source, &destination)? {
            continue;
        }
        // The destination keeps the content, so a failing unlink only leaves a
        // second name that the caller's staging cleanup removes.
        if !keep_source {
            let _ = fs::remove_file(source);
        }
        return Ok(destination);
    }
    unreachable!()
}

/// A short, actionable explanation of any failure, without terminal escapes.
pub fn readable_error(error: &Error) -> String {
    if error.is_cancelled() {
        return "Cancelled.".into();
    }
    if error.is_code("EACCES") || error.is_code("EPERM") {
        return "Permission denied. Choose a writable output directory or check executable permissions.".into();
    }
    if error.is_code("ENOSPC") {
        return "Not enough disk space to save the download.".into();
    }
    let text = clean_text(&error.message);
    if re!(r"(?i)did not get any data blocks").is_match(&text) {
        return "The media stream returned no data (yt-dlp: Did not get any data blocks). The download is not confirmed complete. Retry the download; if it persists, update the backend with veo backend update.".into();
    }
    if re!(r"(?i)unsupported url|no suitable extractor").is_match(&text) {
        return "This URL or website is not supported by the downloading backend.".into();
    }
    if re!(r"(?i)private|login required|sign in|log in|authentication|members.only|not a bot|cookies").is_match(&text) {
        return "This content is private or requires authentication. veo does not bypass access controls. If you are authorized to view it, pass your own session with --cookies <file> or --cookies-from-browser <browser>.".into();
    }
    if re!(r"(?i)deleted|removed|no longer available|404|not found|does not exist").is_match(&text) {
        return "The content was deleted, removed, or could not be found.".into();
    }
    if re!(r"(?i)could not write header|only vp8 or vp9 or av1|not supported in container|could not find tag").is_match(&text) {
        return "The selected codecs do not fit this container. Try --format mkv for lossless output, or add --recode to explicitly convert (may lose quality).".into();
    }
    if re!(r"(?i)requested format|no video formats|no suitable formats|conversion failed|error opening encoder|could not find tag").is_match(&text) {
        return "The requested format is unavailable or could not be converted. Try -q best or another --format.".into();
    }
    if re!(r"(?i)drm.protected|digital rights").is_match(&text) {
        return "This content is DRM-protected. veo does not remove DRM.".into();
    }
    if let Some(captures) = re!(r"(?i)HTTP(?: Error)?\s*:?\s*(403|429|5[0-9][0-9])\b").captures(&text) {
        return match &captures[1] {
            "403" => {
                "HTTP 403 Forbidden: the server refused the media download. The video page may still be accessible. Try veo backend update and retry.".into()
            }
            "429" => "HTTP 429 Too Many Requests: the server is rate-limiting downloads. Wait before retrying.".into(),
            code => format!("HTTP {code}: the server failed to handle the download. Try again later."),
        };
    }
    if re!(r"(?i)network|timed? ?out|connection|resolve|ENOTFOUND|ECONN|fetch failed").is_match(&text) {
        let detail = re!(r"(?i)\b(?:ENOTFOUND|EAI_AGAIN|ECONNRESET|ECONNREFUSED|ETIMEDOUT)\b")
            .find(&text)
            .map(|found| found.as_str().to_string())
            .unwrap_or_else(|| {
                if re!(r"(?i)timed? ?out").is_match(&text) {
                    "request timed out".into()
                } else if re!(r"(?i)connection reset").is_match(&text) {
                    "connection reset".into()
                } else {
                    "connection failed".into()
                }
            });
        return format!("Network request failed ({detail}). Check your connection and try again later.");
    }
    if re!(r"(?i)not available|unavailable|geo.?restrict|country").is_match(&text) {
        return "This content is unavailable or restricted in your region.".into();
    }
    if text.is_empty() {
        return "Download failed. Please try again.".into();
    }
    let chars: Vec<char> = text.chars().collect();
    chars[chars.len().saturating_sub(1200)..].iter().collect()
}

/// Locale-style ordering for short identifiers such as language codes.
pub fn locale_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| b.cmp(a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_untrusted_titles() {
        assert_eq!(sanitize_title("a/b:c*?"), "a_b_c__");
        assert_eq!(sanitize_title("con"), "_con");
        assert_eq!(sanitize_title("COM1.txt"), "_COM1.txt");
        assert_eq!(sanitize_title("  name.. "), "name");
        assert_eq!(sanitize_title("..."), "video");
        assert_eq!(sanitize_title(""), "video");
        assert_eq!(sanitize_title("\u{202e}evil"), "_evil");
        let long = "é".repeat(200);
        assert_eq!(sanitize_title(&long).len(), 180);
    }

    #[test]
    fn cleans_terminal_text() {
        assert_eq!(clean_text("\x1b[31mred\x1b[0m\tline\n"), "red line");
        assert_eq!(clean_text("  \u{feff}x\u{a0}"), "x");
    }

    #[test]
    fn validates_urls() {
        assert_eq!(validate_url("https://example.com").unwrap(), "https://example.com/");
        assert!(validate_url("ftp://example.com").is_err());
        assert!(validate_url("https://user:pass@example.com/").is_err());
        assert!(validate_url("not a url").is_err());
    }

    #[test]
    fn validates_browser_specs() {
        assert_eq!(validate_browser_spec("firefox:default::work").unwrap(), "firefox:default::work");
        assert!(validate_browser_spec("netscape").is_err());
        assert!(validate_browser_spec("chrome+kwallet+basic").is_err());
        assert!(validate_browser_spec("chrome:-x").is_err());
        assert!(validate_browser_spec("chrome+gnomekeyring:Profile 1").is_ok());
    }

    #[test]
    fn selects_heights() {
        let format = |height: f64, vcodec: &str| Format { height: Some(height), vcodec: Some(vcodec.into()), ..Format::default() };
        let formats = vec![format(1080.0, "avc1"), format(720.0, "avc1"), format(480.0, "none"), format(360.0, "vp9")];
        assert_eq!(available_heights(&formats), vec![1080, 720, 360]);
        assert_eq!(capped_height(&formats, "900p"), Some(720));
        assert_eq!(capped_height(&formats, "240p"), None);
        assert_eq!(closest_height(&formats, "900p"), Some(720));
        // 720p and 360p are equally far from 540p; the smaller download wins.
        assert_eq!(closest_height(&formats, "540p"), Some(360));
    }

    #[test]
    fn explains_errors() {
        assert!(readable_error(&Error::msg("ERROR: HTTP Error 429: Too Many Requests")).starts_with("HTTP 429"));
        assert!(readable_error(&Error::msg("ERROR: Unsupported URL: x")).contains("not supported"));
        assert_eq!(readable_error(&Error::cancelled()), "Cancelled.");
        assert!(readable_error(&Error::msg("getaddrinfo ENOTFOUND host")).contains("ENOTFOUND"));
    }
}
