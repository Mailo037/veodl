//! Downloading media with yt-dlp: metadata, staging, transfers and listings.

mod args;
mod collection;
mod deps;
mod keys;
mod listing;
mod native;
mod quality;
mod runner;
mod single;
mod staging;

pub use args::*;
use collection::*;
pub use deps::*;
pub use keys::*;
pub use listing::*;
pub use quality::*;
pub use runner::*;
pub use single::*;
pub use staging::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use serde_json::Map;

    use crate::metadata::{Format, Metadata};
    use crate::options::Options;

    fn heights(values: &[f64]) -> Vec<Format> {
        values.iter().map(|height| Format { height: Some(*height), vcodec: Some("avc1".into()), ..Format::default() }).collect()
    }

    #[test]
    fn quality_is_an_upper_bound() {
        let formats = heights(&[2160.0, 1080.0, 720.0]);
        let choice = select_quality("1440p", false, false, &formats, false);
        assert_eq!(choice.format.as_deref(), Some("bv[height=1080]+ba/b[height=1080]/bv[height=1080]"));
        assert_eq!(choice.label.as_deref(), Some("Quality: 1080p (highest at or below 1440p)"));
        let none = select_quality("480p", false, false, &formats, false);
        assert!(none.error.unwrap().contains("offered: 720p, 1080p, 2160p"));
        let closest = select_quality("1440p", false, true, &formats, false);
        assert_eq!(closest.label.as_deref(), Some("Quality: 1080p (closest to 1440p)"));
        assert_eq!(select_quality("720p", false, false, &formats, true).sort.as_deref(), Some("res:720"));
        assert_eq!(select_quality("720p", true, false, &formats, false), QualityChoice::default());
        assert_eq!(select_quality("720p", false, false, &[], false).label.as_deref(), Some("Resolution unknown; using the best available stream."));
    }

    #[test]
    fn recognizes_staged_files() {
        assert!(is_staged_media("media.mp4"));
        assert!(is_staged_media("media-002.webm"));
        assert!(!is_staged_media("media.f137.mp4"));
        assert!(!is_staged_media("media.jpg"));
        assert!(is_sidecar("media.en.vtt"));
        assert!(!is_sidecar("media.mp4"));
        assert!(!is_sidecar(".lock"));
    }

    #[test]
    fn titles_follow_rename_rules() {
        let metadata = Metadata {
            title: Some("List".into()),
            entries: Some(vec![Some(Metadata { title: Some("First".into()), ..Metadata::default() }), None]),
            ..Metadata::default()
        };
        assert_eq!(staged_title("media-001.mp4", &metadata, None), "First");
        assert_eq!(staged_title("media-002.mp4", &metadata, None), "List - 002");
        assert_eq!(staged_title("media.mp4", &metadata, Some("x *")), "x List");
        assert_eq!(staged_title("media-001.mp4", &metadata, Some("Name")), "Name - 001");
    }

    #[test]
    fn staging_template_escapes_percent() {
        let template = staging_template(Path::new("/tmp/a%b"), true);
        assert!(template.ends_with("a%%b/media-%(playlist_index)03d.%(ext)s") || template.ends_with("a%%b\\media-%(playlist_index)03d.%(ext)s"), "{template}");
    }

    #[test]
    fn settings_keys_are_stable() {
        let options = Options { quality: "best".into(), url: "https://example.com/v".into(), output: "out".into(), ..Options::default() };
        assert_eq!(
            serde_json::to_string(&download_settings(&options)).unwrap(),
            r#"{"quality":"best","audio":false,"format":null,"closestQuality":false,"subs":false,"subLangs":null,"embedSubs":false,"embedMetadata":false,"embedThumbnail":false,"sponsorblockRemove":null,"section":null}"#
        );
        let metadata = Metadata { id: Some("abc".into()), extractor_key: Some("Youtube".into()), ..Metadata::default() };
        assert_eq!(partial_key(&metadata, &options.url, &options).len(), 24);
    }

    #[test]
    fn staging_keys_match_veo_1x() {
        // Values computed with the JavaScript implementation (veo 1.12.1), so
        // retained and resumable downloads of 1.x stay usable after upgrading.
        let args: Vec<String> = ["https://example.com/watch?v=1", "-q", "720p", "-o", "C:/out", "--subs", "--sub-format", "srt", "--format", "mkv"]
            .iter()
            .map(|arg| arg.to_string())
            .collect();
        let options = crate::cli::parse_cli(&args, &Map::new()).unwrap();
        let metadata = Metadata { id: Some("abc".into()), extractor_key: Some("Youtube".into()), ..Metadata::default() };
        assert_eq!(partial_key(&metadata, &options.url, &options), "79f4859f93e33b59eb790ca9");
        if cfg!(windows) {
            assert_eq!(local_request_key(&options), "db4dee629cd61e627a57b5c8");
            assert_eq!(local_request_key(&Options { entry_index: Some(3), ..options }), "fdb1c800cea8bd69b8cffc65");
        }
    }

    #[test]
    fn result_json_keeps_field_order() {
        let result = DownloadResult { url: "u".into(), title: Some("t".into()), files: vec![], status: "saved".into(), saved: 1, ..DownloadResult::default() };
        assert_eq!(serde_json::to_string(&result.to_json()).unwrap(), r#"{"url":"u","title":"t","files":[],"status":"saved","saved":1,"skipped":0}"#);
    }
}
