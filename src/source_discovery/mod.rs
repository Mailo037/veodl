//! Finding playable media on web pages that yt-dlp cannot extract directly.

mod cdp;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::cancel::Cancel;
use crate::downloader::number;
use crate::error::{Error, Result};
use crate::metadata::Metadata;
use crate::paths;
use crate::utils::{clean_text, validate_url};

pub use cdp::*;

pub(super) const MEDIA_URL: &str = r"(?i)\.(?:m3u8|mpd|mp4|webm|mov)(?:[?#]|$)";

pub(super) const MEDIA_TYPE: &str = r"(?i)(?:mpegurl|dash\+xml|video/(?:mp4|webm|quicktime))";

pub fn parse_source_timeout(value: &str) -> Result<u64> {
    let captures = re!(r"(?i)^([0-9]+)(ms|s|m)?$").captures(value.trim()).ok_or_else(|| Error::msg("--timeout expects seconds, e.g. 30, 30s or 2m."))?;
    let factor = match captures.get(2).map(|unit| unit.as_str().to_lowercase()).as_deref() {
        Some("ms") => 1,
        Some("m") => 60_000,
        _ => 1000,
    };
    let milliseconds = captures[1].parse::<u64>().ok().and_then(|value| value.checked_mul(factor));
    match milliseconds {
        Some(milliseconds) if (5000..=600_000).contains(&milliseconds) => Ok(milliseconds),
        _ => Err(Error::msg("--timeout must be between 5 seconds and 10 minutes.")),
    }
}

pub fn should_offer_source_discovery(error: &Error) -> bool {
    if error.is_cancelled() {
        return false;
    }
    !re!(r"(?i)(?:HTTP (?:Error )?(?:401|403|404|429|5[0-9][0-9])|\b(?:DRM|live stream|rate limit|timed out|timeout|connection refused|login required|sign in|private video|geo.?block)\b|This URL is a collection)").is_match(&error.message)
}

pub fn browser_candidates() -> Vec<PathBuf> {
    if let Some(custom) = std::env::var_os("VEO_BROWSER_PATH").filter(|value| !value.is_empty()) {
        return vec![PathBuf::from(custom)];
    }
    match paths::platform() {
        "win32" => ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"]
            .iter()
            .filter_map(|name| std::env::var_os(name).filter(|value| !value.is_empty()))
            .flat_map(|root| {
                let root = PathBuf::from(root);
                [
                    root.join("Google").join("Chrome").join("Application").join("chrome.exe"),
                    root.join("Microsoft").join("Edge").join("Application").join("msedge.exe"),
                    root.join("BraveSoftware").join("Brave-Browser").join("Application").join("brave.exe"),
                ]
            })
            .collect(),
        "darwin" => [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        ]
        .iter()
        .map(PathBuf::from)
        .collect(),
        _ => ["chromium", "chromium-browser", "google-chrome", "microsoft-edge", "brave-browser"].iter().map(PathBuf::from).collect(),
    }
}

pub fn media_urls_from_text(body: &str) -> Vec<String> {
    if body.len() > 524_288 {
        return Vec::new();
    }
    let normalized = body.replace("\\/", "/").replace("\\u0026", "&").replace("&amp;", "&");
    let mut seen = HashSet::new();
    re!(r#"(?i)https?://[^\s"'<>\\]+"#)
        .find_iter(&normalized)
        .map(|found| found.as_str().trim_end_matches([')', ',', ';']).to_string())
        .filter(|value| re!(MEDIA_URL).is_match(value))
        .filter_map(|value| validate_url(&value).ok())
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

/// One verified media source found on a page.
#[derive(Clone, Debug)]
pub struct Source {
    pub index: usize,
    pub title: String,
    pub host: String,
    pub kind: String,
    pub quality: Vec<String>,
    pub estimated_mib: Option<f64>,
    pub bitrate_mbps: Option<f64>,
    pub url: String,
}

impl Source {
    /// JSON without the media and page URLs (which may be signed).
    pub fn safe_json(&self) -> Value {
        json!({
            "index": self.index, "title": self.title, "source": self.host, "type": self.kind, "quality": self.quality,
            "estimatedMiB": self.estimated_mib.map_or(Value::Null, number), "bitrateMbps": self.bitrate_mbps.map_or(Value::Null, number),
        })
    }
}

pub fn summarize_source(metadata: &Metadata, url: &str, index: usize) -> Source {
    let formats: Vec<_> = metadata.formats().iter().filter(|format| !format.drm()).collect();
    let mut heights: Vec<u64> = formats.iter().filter_map(|format| format.height.filter(|height| height.is_finite())).map(|height| height as u64).collect();
    heights.sort_unstable_by(|a, b| b.cmp(a));
    heights.dedup();
    let bytes = metadata.filesize.filter(|value| *value != 0.0).or(metadata.filesize_approx.filter(|value| *value != 0.0));
    let bitrate = match metadata.tbr.filter(|tbr| tbr.is_finite()) {
        Some(tbr) => tbr,
        None => formats.iter().filter_map(|format| format.tbr.filter(|tbr| tbr.is_finite())).fold(0.0, f64::max),
    };
    let host = url::Url::parse(url).ok().and_then(|url| url.host_str().map(str::to_string)).unwrap_or_default();
    Source {
        index,
        title: clean_text(metadata.title.as_deref().filter(|title| !title.is_empty()).or(metadata.id.as_deref()).unwrap_or("Video")),
        host,
        kind: if re!(r"(?i)\.m3u8(?:[?#]|$)").is_match(url) {
            "HLS"
        } else if re!(r"(?i)\.mpd(?:[?#]|$)").is_match(url) {
            "DASH"
        } else {
            "Video"
        }
        .into(),
        quality: heights.iter().map(|height| format!("{height}p")).collect(),
        estimated_mib: bytes.map(|bytes| (bytes / 1_048_576.0 * 10.0).round() / 10.0),
        bitrate_mbps: (bitrate > 0.0).then(|| (bitrate / 100.0).round() / 10.0),
        url: url.into(),
    }
}

pub fn format_source(source: &Source) -> String {
    let size = source.estimated_mib.map_or("Size unknown".to_string(), |mib| format!("about {} MiB", number(mib)));
    let bitrate = source.bitrate_mbps.map_or(String::new(), |rate| format!(" | {} Mbit/s", number(rate)));
    let quality = if source.quality.is_empty() { "Quality unknown".to_string() } else { source.quality.join(", ") };
    format!("{}. {} | {} | {} | {quality} | {size}{bitrate}", source.index, source.title, source.host, source.kind)
}

pub type Inspect<'a> = &'a dyn Fn(&str, &Cancel) -> Result<Metadata>;

pub type Capturer<'a> = &'a dyn Fn(&str, &Cancel, Duration, bool) -> Result<Vec<String>>;

pub struct Discovered {
    pub sources: Vec<Source>,
    pub timed_out: bool,
}

pub(super) fn media_key(value: &str) -> Option<String> {
    url::Url::parse(value).ok().map(|url| format!("{}{}", url.origin().ascii_serialization(), url.path()))
}

/// Verify captured candidates with yt-dlp before offering them.
pub fn discover_sources(page_url: &str, cancel: &Cancel, deep: bool, timeout_ms: Option<u64>, inspect: Inspect, capture: Capturer) -> Result<Discovered> {
    cancel.check()?;
    let timeout_ms = timeout_ms.unwrap_or(if deep { 120_000 } else { 45_000 });
    let scan = cancel.child_with_timeout(Duration::from_millis(timeout_ms));
    let observe = Duration::from_millis(if deep { 30_000.min(timeout_ms * 6 / 10) } else { 8_000.min(timeout_ms / 2) });
    let urls = match capture(page_url, &scan, observe, deep) {
        Ok(urls) => urls,
        Err(_) if scan.timed_out() => Vec::new(),
        Err(error) => return Err(error),
    };
    cancel.check()?;
    let mut sources: Vec<Source> = Vec::new();
    let mut seen = HashSet::new();
    let mut references: HashMap<String, HashSet<String>> = HashMap::new();
    for url in urls.iter().take(if deep { usize::MAX } else { 30 }) {
        if scan.timed_out() {
            break;
        }
        cancel.check()?;
        let Ok(metadata) = inspect(url, &scan) else { continue };
        let formats = metadata.formats();
        if metadata.drm()
            || metadata.live()
            || (!formats.is_empty() && formats.iter().all(|format| format.drm()))
            || (formats.is_empty() && metadata.url.is_none())
        {
            continue;
        }
        let Some(address) = media_key(url) else { continue };
        if !seen.insert(format!("{address}:{}", metadata.id.as_deref().unwrap_or(""))) {
            continue;
        }
        sources.push(summarize_source(&metadata, url, sources.len() + 1));
        references.insert(url.clone(), formats.iter().filter_map(|format| format.url.as_deref().and_then(media_key)).collect());
    }
    cancel.check()?;
    // A master playlist and its variant streams describe the same video.
    let keys: Vec<Option<String>> = sources.iter().map(|source| media_key(&source.url)).collect();
    let mut distinct: Vec<Source> = sources
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            !sources.iter().enumerate().any(|(other, source)| {
                other != *index && keys[*index].as_ref().is_some_and(|key| references.get(&source.url).is_some_and(|set| set.contains(key)))
            })
        })
        .map(|(_, source)| source.clone())
        .collect();
    for (index, source) in distinct.iter_mut().enumerate() {
        source.index = index + 1;
    }
    Ok(Discovered { sources: distinct, timed_out: scan.timed_out() })
}

pub fn default_capture(page_url: &str, cancel: &Cancel, observe: Duration, deep: bool) -> Result<Vec<String>> {
    capture_browser_media(page_url, cancel, observe, deep)
}

pub fn sources_json(url: &str, discovered: &Discovered) -> Value {
    let mut map = Map::new();
    map.insert("url".into(), Value::String(url.into()));
    map.insert("status".into(), Value::String("sources".into()));
    map.insert("timedOut".into(), Value::Bool(discovered.timed_out));
    map.insert("sources".into(), Value::Array(discovered.sources.iter().map(Source::safe_json).collect()));
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_timeouts() {
        assert_eq!(parse_source_timeout("30").unwrap(), 30_000);
        assert_eq!(parse_source_timeout("2m").unwrap(), 120_000);
        assert!(parse_source_timeout("1s").is_err());
        assert!(parse_source_timeout("11m").is_err());
        assert!(parse_source_timeout("abc").is_err());
    }

    #[test]
    fn extracts_media_urls_from_text() {
        let body = r#"{"src":"https:\/\/cdn.example.com\/v\/master.m3u8?a=1&b=2","img":"https://x.example.com/a.png"}"#;
        assert_eq!(media_urls_from_text(body), vec!["https://cdn.example.com/v/master.m3u8?a=1&b=2"]);
    }

    #[test]
    fn decides_when_to_search() {
        assert!(should_offer_source_discovery(&Error::msg("ERROR: Unsupported URL: https://x")));
        assert!(!should_offer_source_discovery(&Error::msg("ERROR: HTTP Error 403: Forbidden")));
        assert!(!should_offer_source_discovery(&Error::msg("This URL is a collection. Add --playlist")));
    }

    #[test]
    fn verifies_and_deduplicates_sources() {
        let capture = |_: &str, _: &Cancel, _: Duration, _: bool| {
            Ok(vec![
                "https://cdn.example.com/master.m3u8".to_string(),
                "https://cdn.example.com/720.m3u8".to_string(),
                "https://cdn.example.com/drm.mpd".to_string(),
            ])
        };
        let inspect = |url: &str, _: &Cancel| -> Result<Metadata> {
            let json = if url.ends_with("master.m3u8") {
                r#"{"id":"m","title":"Clip","formats":[{"height":720,"tbr":2500,"url":"https://cdn.example.com/720.m3u8"}]}"#
            } else if url.ends_with("720.m3u8") {
                r#"{"id":"v","title":"Clip","formats":[{"height":720}]}"#
            } else {
                r#"{"id":"d","has_drm":true,"formats":[{"height":1080}]}"#
            };
            Ok(serde_json::from_str(json).unwrap())
        };
        let found = discover_sources("https://example.com/page", &Cancel::new(), false, Some(10_000), &inspect, &capture).unwrap();
        assert_eq!(found.sources.len(), 1);
        assert_eq!(format_source(&found.sources[0]), "1. Clip | cdn.example.com | HLS | 720p | Size unknown | 2.5 Mbit/s");
        assert!(!found.timed_out);
    }
}
