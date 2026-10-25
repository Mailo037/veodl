use std::sync::OnceLock;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

const SOURCE_OPTIONS_GUIDE: &str = r#"// veo: source discovery options v1
// Copy these settings into the global object or a named profile if desired.
// "deepScan": true,          // Verify every observed media candidate instead of the first 30
// "timeout": "2m",          // Discovery deadline: 5s to 10m; plain numbers mean seconds
// "listSources": false,     // true = list sources and exit for single-URL calls
// "autoListSources": true,  // Search after extraction fails; false disables fallback
// Command-line flags override these values, including --no-deep-scan and --no-list-sources.
"#;

const PROFILE_OPTIONS_GUIDE: &str = r#"// veo: profile download options v6
// Any global option can also be placed inside a profile.
// Copy the settings you want into "profiles" > "default" or a named profile.
// Remove the leading // on copied settings; separate entries with commas.
// Speed without reducing the selected video quality:
// "quality": "best",
// "concurrentDownloads": 2,    // Parallel URLs/batch items: 1-4; default 2
// "adaptiveConcurrency": true, // Host-local back-off and gradual recovery
// "concurrentFragments": 8,    // DASH/HLS fragments: 1-64; default 8
// Values 17-64 are experimental and require terminal confirmation before download.
// "experimentalFragments": false, // true explicitly accepts 17-64, including in scripts
// More connections may trigger server limits and do not guarantee better speed.
// "nativeHttp": true,        // Rust downloader for eligible direct HTTP media
// "httpConnections": 4,      // 1-16; shares a host budget with other videos/fragments
// "stagingDir": "output",    // Stage beside the destination; or choose a directory
// Default staging remains the local cache; prefer it for slow network drives.
// "fastCut": false,          // With section: true cuts at nearby keyframes, less precisely
// "deepScan": true,           // Check every observed source candidate
// "timeout": "2m",           // Source-search deadline, 5s to 10m
// "listSources": false,      // List sources and exit for one URL
// "autoListSources": true,   // Search after extraction fails (default)
// "playlist": true,            // Enable only for playlist downloads
// "playlistConcurrency": 2,    // Simultaneous entries: 1-4; default 2
// "checkSpace": true,          // Estimate free cache/output space first
// "timings": true,             // Show time spent in each phase
// "color": true,               // Gray details and colored results; false = plain text
// Applies to veo's own terminal output, including stats/history/doctor.
// Each profile can set color independently; NO_COLOR always disables styling.
// Example inside profiles: "plain": { "color": false }
// Use: veo stats --profile plain or veo URL --profile plain
// "folderTemplate": "{channel}/{year}",
// "filenameTemplate": "{index} - {title}", // Without extension; do not combine with rename
// "resume": true,
// "incognito": false,        // Save only media; no run ID, history, stats or retry data
// "neutralFilename": false,  // Optional random filenames; ignores rename and naming templates
// "skipExisting": true
// "verify": false,           // Probe saved media with FFprobe; no full decode
// Lossless container change (the selected codecs must fit the container):
// "format": "mkv",
// "compatible": false,        // Opt-in H.264/AAC MP4; converts only when needed
// "recode": false
// Compatibility profile to add inside profiles:
// "kompatibel": { "audio": false, "format": "mp4", "compatible": true, "recode": false }
// Use: veo URL --profile kompatibel (or simply veo URL --compatible).
// Optional conversion instead: "format": "webm", "recode": true
// Conversion can be slower and lose quality; recode is for video only.
// Sequential downloads: "concurrentDownloads": 1, "concurrentFragments": 1, "playlistConcurrency": 1
// Example named profile inside the profiles object:
// "fast": { "quality": "best", "concurrentFragments": 8, "playlistConcurrency": 2, "resume": true }
// Use with: veo URL --profile fast (add --playlist for a playlist URL).
// Inspect: veo config show --profile fast | Validate: veo config check
// Template fields: {title}, {id}, {channel}, {year}, {playlist}, {index}.
// Missing values use Unknown channel/year, No playlist, unknown ID, index 001.
"#;

fn template_source() -> &'static str {
    static SOURCE: OnceLock<String> = OnceLock::new();
    SOURCE.get_or_init(|| {
        format!(
            r#"// veo: commented configuration template
// Save and close the editor. Settings apply the next time you run veo.
// Comments using // or /* ... */ are supported. Command-line options take priority.
// To enable an option, remove its leading // and adjust the value.
// Separate active entries with commas. Do not add a comma after the last entry.
{{
  // Output directory: use / or double backslashes, for example "D:\\Videos".
  // Without output, downloads are saved in the current working directory.
  // "output": "D:/Videos",
  // Filename without extension. Every * inserts the original video title.
  // "rename": "movie_*",

  // Maximum video resolution: best, 2160p, 1440p, 1080p, 720p, ...
  // "quality": "1080p",
  // "closestQuality": false, // Allow the nearest available resolution
  // Video format: mp4, mkv, webm, mov. Lossless remux; codecs must fit the container.
  // "format": "mp4",
  // "recode": false,           // Explicit video conversion; may lose quality
  // "playlistConcurrency": 2, // Concurrent playlist entries: 1 to 4
  // "playlistItems": "1-5",  // Download only these playlist entries
  // "section": "*10:00-12:00", // Download only a time range
  // "open": false,              // Open the completed file automatically
  // "resume": true,             // Resume interrupted downloads
  // "incognito": true,          // Save only media; no run ID, history, stats or retry data
  // "neutralFilename": false,  // Set true for random filenames without title or template folders
  // "skipExisting": true,       // Skip previously saved downloads
  // "verify": false,           // Check saved media container and tracks with FFprobe
  // "concurrentFragments": 8,   // Concurrent fragments: 1 to 64; default 8
  // Values above 16 require confirmation or the explicit opt-in below.
  // "experimentalFragments": false, // true accepts experimental values 17-64
  // "cookies": "D:/cookies.txt", // Netscape-format cookies file
  // "cookiesFromBrowser": "firefox", // Use your browser's cookies
  // "sponsorblockRemove": "sponsor", // Remove matching SponsorBlock segments
  // "json": false,            // Print machine-readable output for downloads

{SOURCE_OPTIONS_GUIDE}

  // Subtitles and additional information:
  // "subs": true,              // Download subtitles; defaults to English
  // "autoSubs": false,         // Include automatically generated subtitles
  // "subLangs": "de,en",        // Enable subtitles for these languages
  // "subFormat": "srt/vtt/best", // Prefer these available subtitle formats
  // "embedSubs": true,          // Embed subtitles in the video
  // "embedMetadata": true,      // Embed the title, date and other metadata
  // "embedThumbnail": true,     // Embed the thumbnail

  // The default profile is used automatically unless you select another profile.
  // Use veo profile NAME to make any named profile the default for future commands.
  // The active choice is stored at the top level: "activeProfile": "music",
{PROFILE_OPTIONS_GUIDE}
  // Select other profiles with --profile NAME or in the interactive wizard.
  // Example: veo "https://example.com/video.mp4" --profile music
  "profiles": {{
    "default": {{
      // "color": true, // Set false to disable terminal styling for this profile.
      // Add everyday defaults here, for example: "quality": "1080p"
      // Empty means use the global settings above and veo's built-in defaults.
    }},
    "kompatibel": {{
      "audio": false,
      "format": "mp4",
      "compatible": true,
      "recode": false
    }},
    "music": {{
      // Audio only. Formats: mp3, m4a, aac, opus, flac, wav.
      "audio": true,
      "format": "mp3"
    }},
    "archive": {{
      "quality": "1080p",
      "embedMetadata": true,
      "subLangs": "de,en"
    }}
  }}
}}
"#
        )
    })
}

const REFERENCE_BEGIN: &str = "// veo: template reference begin";
const REFERENCE_END: &str = "// veo: template reference end";

pub const CONFIG_TEMPLATE: &str = r#"{
  "profiles": {
    "default": {},
    "kompatibel": { "audio": false, "format": "mp4", "compatible": true, "recode": false },
    "music": { "audio": true, "format": "mp3" },
    "archive": { "quality": "1080p", "embedMetadata": true, "subLangs": "de,en" }
  }
}
"#;

// The guide revision follows its content so the editor can offer an update.
fn revision() -> &'static str {
    static REVISION: OnceLock<String> = OnceLock::new();
    REVISION.get_or_init(|| hex::encode(Sha256::digest(template_source().as_bytes()))[..12].to_string())
}

/// The commented option reference, inserted into a config only when requested.
pub fn config_guide() -> &'static str {
    static GUIDE: OnceLock<String> = OnceLock::new();
    GUIDE.get_or_init(|| {
        let body = template_source()
            .split('\n')
            .skip(1)
            .map(|line| if line.starts_with("//") { line.to_string() } else { format!("// {line}") })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "{REFERENCE_BEGIN} {}\n// Current veo options. Copy only the settings you want into the JSON below.\n{}\n{REFERENCE_END}\n",
            revision(),
            body.trim_end()
        )
    })
}

struct Range {
    start: usize,
    stop: usize,
    revision: String,
}

fn generated_range(text: &str) -> Option<Range> {
    let Some(begin) = text.find(REFERENCE_BEGIN) else {
        let legacy = crate::re!(r"^\x{FEFF}?// veo: commented configuration template\r?\n// Reference: copy any examples");
        let marker = if legacy.is_match(text) { text.find("// Your existing settings:") } else { None };
        let marker = marker?;
        let stop = text[marker..].find('\n').map_or(text.len(), |offset| marker + offset + 1);
        return Some(Range { start: 0, stop, revision: "legacy".into() });
    };
    let end = begin + text[begin..].find(REFERENCE_END)?;
    let before = &text[..begin];
    let start = if crate::re!(r"^\x{FEFF}?// veo: commented configuration template\r?\n$").is_match(before) { 0 } else { begin };
    let mut stop = text[end..].find('\n').map_or(text.len(), |offset| end + offset + 1);
    if text[stop..].starts_with("// Your existing settings:\n") {
        stop += "// Your existing settings:\n".len();
    }
    let line_end = text[begin..].find('\n').map_or(text.len(), |offset| begin + offset);
    Some(Range { start, stop, revision: text[begin..line_end].to_string() })
}

/// absent, incomplete, outdated or current.
pub fn config_guide_state(text: &str) -> &'static str {
    match generated_range(text) {
        None if text.contains(REFERENCE_BEGIN) || text.contains("// Reference: copy any examples") => "incomplete",
        None => "absent",
        Some(range) if range.revision == format!("{REFERENCE_BEGIN} {}", revision()) => "current",
        Some(_) => "outdated",
    }
}

pub fn add_config_guide(text: &str) -> String {
    let range = generated_range(text);
    if range.is_none() && config_guide_state(text) == "incomplete" {
        return text.to_string();
    }
    let bom = if text.starts_with('\u{feff}') { "\u{feff}" } else { "" };
    match range {
        Some(range) => format!("{}{}{}", if range.start == 0 { bom } else { &text[..range.start] }, config_guide(), &text[range.stop..]),
        None => format!("{bom}{}{}", config_guide(), &text[bom.len()..]),
    }
}

pub fn remove_config_guide(text: &str) -> String {
    match generated_range(text) {
        Some(range) => {
            let head = if range.start == 0 && text.starts_with('\u{feff}') { "\u{feff}" } else { &text[..range.start] };
            format!("{head}{}", &text[range.stop..])
        }
        None => text.to_string(),
    }
}

/// Remove comments only outside JSON strings. Every comment byte becomes a
/// space (line breaks stay), so offsets are preserved and malformed input
/// cannot become valid by joining tokens. A leading BOM is dropped.
pub fn strip_config_comments(text: &str) -> Result<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let bytes = text.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut quoted = false;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if quoted {
            result.push(byte);
            if byte == b'\\' && index + 1 < bytes.len() {
                index += 1;
                result.push(bytes[index]);
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
            result.push(byte);
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            result.extend_from_slice(b"  ");
            index += 1;
            while index + 1 < bytes.len() && bytes[index + 1] != b'\r' && bytes[index + 1] != b'\n' {
                result.push(b' ');
                index += 1;
            }
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            let start = index;
            result.extend_from_slice(b"  ");
            index += 1;
            let mut closed = false;
            while index + 1 < bytes.len() {
                index += 1;
                if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    result.extend_from_slice(b"  ");
                    index += 1;
                    closed = true;
                    break;
                }
                result.push(if bytes[index] == b'\r' || bytes[index] == b'\n' { bytes[index] } else { b' ' });
            }
            if !closed {
                return Err(Error { position: Some(start), ..Error::msg("Unterminated block comment; add */ to close it.") });
            }
        } else {
            result.push(byte);
        }
        index += 1;
    }
    Ok(String::from_utf8(result).unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_comments_outside_strings() {
        let text = "{\n  // note\n  \"a\": \"//x\", /* b */ \"c\": 1\n}";
        let clean = strip_config_comments(text).unwrap();
        assert_eq!(clean.len(), text.len());
        let value: serde_json::Value = serde_json::from_str(&clean).unwrap();
        assert_eq!(value["a"], "//x");
        assert!(strip_config_comments("{ /* open").is_err());
    }

    #[test]
    fn guide_round_trips() {
        let with = add_config_guide(CONFIG_TEMPLATE);
        assert_eq!(config_guide_state(&with), "current");
        assert_eq!(remove_config_guide(&with), CONFIG_TEMPLATE);
        assert_eq!(config_guide_state(CONFIG_TEMPLATE), "absent");
        assert!(serde_json::from_str::<serde_json::Value>(&strip_config_comments(&with).unwrap()).is_ok());
    }
}
