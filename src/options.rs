use serde_json::{Map, Value};

/// Every resolved setting of one download request. Field names map to the
/// camelCase keys of the config file and of saved retry jobs.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub quality: String,
    pub output: String,
    pub rename: Option<String>,
    pub format: Option<String>,
    pub compatible: bool,
    pub recode: bool,
    pub concurrent_downloads: u32,
    pub filename_template: Option<String>,
    pub folder_template: Option<String>,
    pub adaptive_concurrency: bool,
    pub check_space: bool,
    pub timings: bool,
    pub color: bool,
    pub playlist_concurrency: u32,
    pub cookies: Option<String>,
    pub cookies_from_browser: Option<String>,
    pub concurrent_fragments: u32,
    pub experimental_fragments: bool,
    pub sub_langs: Option<String>,
    pub sponsorblock_remove: Option<String>,
    pub section: Option<String>,
    pub fast_cut: bool,
    pub staging_dir: Option<String>,
    pub native_http: bool,
    pub http_connections: u32,
    pub open: bool,
    pub audio: bool,
    pub resume: bool,
    pub incognito: bool,
    pub neutral_filename: bool,
    pub playlist: bool,
    pub subs: bool,
    pub auto_subs: bool,
    pub sub_format: Option<String>,
    pub verify: bool,
    pub embed_subs: bool,
    pub embed_metadata: bool,
    pub embed_thumbnail: bool,
    pub closest_quality: bool,
    pub json: bool,
    pub skip_existing: bool,
    pub playlist_items: Option<String>,
    pub source: Option<String>,
    pub timeout: Option<String>,
    pub deep_scan: bool,
    pub list_sources: bool,
    pub auto_list_sources: bool,
    pub list_qualities: bool,
    pub list_formats: bool,
    pub list_subs: bool,
    pub dry_run: bool,
    pub batch_file: Option<String>,
    pub retry_failed: Option<String>,
    pub help: bool,
    pub help_all: bool,
    pub version: bool,
    pub urls: Vec<String>,
    pub url: String,
    pub timeout_ms: Option<u64>,
    pub profile: Option<String>,
    /// A media URL found by source discovery for this page.
    pub media_url: Option<String>,
    /// Position inside a playlist while one entry downloads.
    pub entry_index: Option<u64>,
    /// Direct entry page from a flat playlist, preserving the parent request identity.
    pub entry_url: Option<String>,
    pub playlist_title: Option<String>,
}

fn text(value: &Option<String>) -> Value {
    value.as_ref().map_or(Value::Null, |text| Value::String(text.clone()))
}

impl Options {
    /// The value of one config/option key, or None when unset (JS undefined).
    pub fn get(&self, key: &str) -> Option<Value> {
        let flag = |value: bool| Some(Value::Bool(value));
        let some = |value: &Option<String>| value.as_ref().map(|text| Value::String(text.clone()));
        match key {
            "quality" => Some(Value::String(self.quality.clone())),
            "output" => Some(Value::String(self.output.clone())),
            "rename" => some(&self.rename),
            "format" => some(&self.format),
            "compatible" => flag(self.compatible),
            "recode" => flag(self.recode),
            "concurrentDownloads" => Some(self.concurrent_downloads.into()),
            "filenameTemplate" => some(&self.filename_template),
            "folderTemplate" => some(&self.folder_template),
            "adaptiveConcurrency" => flag(self.adaptive_concurrency),
            "checkSpace" => flag(self.check_space),
            "timings" => flag(self.timings),
            "color" => flag(self.color),
            "playlistConcurrency" => Some(self.playlist_concurrency.into()),
            "cookies" => some(&self.cookies),
            "cookiesFromBrowser" => some(&self.cookies_from_browser),
            "concurrentFragments" => Some(self.concurrent_fragments.into()),
            "experimentalFragments" => flag(self.experimental_fragments),
            "subLangs" => some(&self.sub_langs),
            "sponsorblockRemove" => some(&self.sponsorblock_remove),
            "section" => some(&self.section),
            "fastCut" => flag(self.fast_cut),
            "stagingDir" => some(&self.staging_dir),
            "nativeHttp" => flag(self.native_http),
            "httpConnections" => Some(self.http_connections.into()),
            "open" => flag(self.open),
            "audio" => flag(self.audio),
            "resume" => flag(self.resume),
            "incognito" => flag(self.incognito),
            "neutralFilename" => flag(self.neutral_filename),
            "playlist" => flag(self.playlist),
            "subs" => flag(self.subs),
            "autoSubs" => flag(self.auto_subs),
            "subFormat" => some(&self.sub_format),
            "verify" => flag(self.verify),
            "embedSubs" => flag(self.embed_subs),
            "embedMetadata" => flag(self.embed_metadata),
            "embedThumbnail" => flag(self.embed_thumbnail),
            "closestQuality" => flag(self.closest_quality),
            "json" => flag(self.json),
            "skipExisting" => flag(self.skip_existing),
            "playlistItems" => some(&self.playlist_items),
            "source" => some(&self.source),
            "timeout" => some(&self.timeout),
            "deepScan" => flag(self.deep_scan),
            "listSources" => flag(self.list_sources),
            "autoListSources" => flag(self.auto_list_sources),
            "listQualities" => flag(self.list_qualities),
            "listFormats" => self.list_formats.then_some(Value::Bool(true)),
            "listSubs" => self.list_subs.then_some(Value::Bool(true)),
            "dryRun" => self.dry_run.then_some(Value::Bool(true)),
            "timeoutMs" => self.timeout_ms.map(Value::from),
            _ => None,
        }
    }

    /// Settings safe to persist in a retry job: no cookies, browser sessions,
    /// profile names, URLs or discovered media addresses.
    pub fn public(&self) -> Map<String, Value> {
        const KEYS: [&str; 53] = [
            "fastCut",
            "stagingDir",
            "nativeHttp",
            "httpConnections",
            "quality",
            "output",
            "rename",
            "format",
            "compatible",
            "recode",
            "concurrentDownloads",
            "filenameTemplate",
            "folderTemplate",
            "adaptiveConcurrency",
            "checkSpace",
            "timings",
            "color",
            "playlistConcurrency",
            "concurrentFragments",
            "experimentalFragments",
            "subLangs",
            "sponsorblockRemove",
            "section",
            "open",
            "audio",
            "resume",
            "incognito",
            "neutralFilename",
            "playlist",
            "subs",
            "autoSubs",
            "subFormat",
            "verify",
            "embedSubs",
            "embedMetadata",
            "embedThumbnail",
            "closestQuality",
            "json",
            "skipExisting",
            "playlistItems",
            "source",
            "timeout",
            "deepScan",
            "listSources",
            "autoListSources",
            "listQualities",
            "listFormats",
            "listSubs",
            "dryRun",
            "timeoutMs",
            "cookies",
            "cookiesFromBrowser",
            "profile",
        ];
        let mut map = Map::new();
        for key in KEYS {
            if matches!(key, "cookies" | "cookiesFromBrowser" | "profile") {
                continue;
            }
            if let Some(value) = self.get(key) {
                map.insert(key.into(), value);
            }
        }
        map.insert("output".into(), Value::String(crate::paths::display(crate::paths::resolve(&self.output))));
        if let Some(directory) = self.staging_dir.as_deref().filter(|directory| *directory != "output") {
            map.insert("stagingDir".into(), Value::String(crate::paths::display(crate::paths::resolve(directory))));
        }
        map
    }

    pub fn profile_value(&self) -> Value {
        text(&self.profile)
    }

    pub fn with_url(&self, url: &str) -> Options {
        Options { url: url.to_string(), ..self.clone() }
    }
}
