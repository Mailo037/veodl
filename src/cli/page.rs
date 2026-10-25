//! Finding the media of a page: direct metadata or source discovery.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::backend::Backend;
use crate::cancel::Cancel;
use crate::downloader::{self, BackendResolver};
use crate::error::{Error, Result};
use crate::metadata::Metadata;
use crate::options::Options;
use crate::output::Out;
use crate::progress::Reporter;
use crate::source_discovery::{self, format_source, should_offer_source_discovery, sources_json, Discovered};
use crate::terminal::{self, Std};

/// Results of searching a page for media.
#[derive(Default)]
pub struct PageSource {
    pub media_url: Option<String>,
    pub source: Option<usize>,
    pub discovered: Option<Discovered>,
    pub needs_selection: bool,
}

pub(super) type Prefetched = Mutex<HashMap<String, Metadata>>;

pub fn prefetch_key(options: &Options) -> String {
    format!("{}\n{}\n{}", options.url, options.media_url.as_deref().unwrap_or(""), options.playlist)
}

/// For a single page, fall back to media requests observed while its player
/// loads. A completed local transfer skips all network work.
pub fn resolve_page_source(
    options: &Options,
    cancel: &Cancel,
    stderr: &Out,
    reporter: &dyn Reporter,
    backend: &BackendResolver,
    prefetched: &Prefetched,
    interactive: bool,
) -> Result<PageSource> {
    let listing = options.dry_run || options.list_sources || options.list_formats || options.list_qualities || options.list_subs;
    if !options.incognito
        && !listing
        && downloader::read_ready_transfer(
            options,
            &crate::download_cache::root_for(options, &crate::download_cache::root()),
            cancel,
            None,
            crate::state::now_ms(),
        )?
        .is_some()
    {
        return Ok(PageSource::default());
    }
    let backend: Backend = backend(cancel, reporter)?;
    let inspect =
        |request: &Options, scan: &Cancel| downloader::fetch_metadata(request, &backend, &downloader::run_backend, scan, &crate::progress::NullReporter);
    let mut original: Option<Error> = None;
    if options.source.is_none() && !options.list_sources {
        match inspect(options, cancel) {
            Ok(metadata) if metadata.has_entries() || !metadata.formats().is_empty() || metadata.url.is_some() => {
                prefetched.lock().unwrap().insert(prefetch_key(options), metadata);
                return Ok(PageSource::default());
            }
            Ok(_) => original = Some(Error::msg("No downloadable video formats found.")),
            Err(error) => original = Some(error),
        }
        let error = original.clone().unwrap();
        // A transient failure is retried with back-off by the download itself.
        if options.adaptive_concurrency && !listing && crate::execution::retryable(&error) {
            return Ok(PageSource::default());
        }
        if !should_offer_source_discovery(&error) {
            return Err(error);
        }
        if !options.auto_list_sources {
            return Err(Error::msg(format!("{} Use --list-sources to search this page, then --source <number> to select one.", error.message)));
        }
        stderr.write("veo: No direct video found; searching the page for a playable source…\n");
    }
    let candidate = |url: &str, scan: &Cancel| inspect(&Options { media_url: Some(url.to_string()), ..options.clone() }, scan);
    let discovered =
        match source_discovery::discover_sources(&options.url, cancel, options.deep_scan, options.timeout_ms, &candidate, &source_discovery::default_capture) {
            Ok(discovered) => discovered,
            Err(error) => {
                if cancel.is_cancelled() || error.is_cancelled() {
                    return Err(error);
                }
                if let Some(original) = &original {
                    return Err(Error::msg(format!("{} Source discovery also failed: {}", original.message, error.message)));
                }
                return Err(error);
            }
        };
    if options.list_sources {
        return Ok(PageSource { discovered: Some(discovered), ..PageSource::default() });
    }
    if discovered.timed_out {
        stderr.write(&format!("veo: Source search timed out after {}; showing verified sources found so far.\n", timeout_label(options)));
    }
    if discovered.sources.is_empty() {
        return Err(original.unwrap_or_else(|| Error::msg("No downloadable video sources were found on this page.")));
    }
    let mut index: usize = options.source.as_deref().and_then(|source| source.parse().ok()).unwrap_or(0);
    if index == 0 {
        if discovered.sources.len() == 1 {
            index = 1;
        } else if !interactive {
            return Ok(PageSource { discovered: Some(discovered), needs_selection: true, ..PageSource::default() });
        }
    }
    if index == 0 {
        for source in &discovered.sources {
            stderr.write(&format!("{}\n", format_source(source)));
        }
        let answer = terminal::ask("Source number (Enter to cancel): ", Std::Err)?;
        if !re!(r"^[1-9][0-9]*$").is_match(answer.trim()) {
            return Err(Error::msg("No source selected."));
        }
        index = answer.trim().parse().unwrap_or(0);
    }
    let selected = discovered
        .sources
        .get(index.wrapping_sub(1))
        .ok_or_else(|| Error::msg(format!("Source {index} does not exist; choose 1-{}.", discovered.sources.len())))?;
    Ok(PageSource { media_url: Some(selected.url.clone()), source: Some(index), ..PageSource::default() })
}

pub(super) fn timeout_label(options: &Options) -> String {
    options.timeout.clone().unwrap_or_else(|| if options.deep_scan { "120s".into() } else { "45s".into() })
}

pub(super) fn print_sources(stdout: &Out, url: &str, discovered: &Discovered, json: bool) {
    if json {
        stdout.line(sources_json(url, discovered).to_string());
    } else {
        for source in &discovered.sources {
            stdout.line(format_source(source));
        }
    }
}

/// Backends prepared during page resolution, reused by the download.
pub(super) struct SharedBackend {
    pub(super) prepared: Mutex<Option<Backend>>,
}

impl SharedBackend {
    pub(super) fn resolve(&self, cancel: &Cancel, reporter: &dyn Reporter) -> Result<Backend> {
        let mut prepared = self.prepared.lock().unwrap_or_else(|poison| poison.into_inner());
        if let Some(backend) = prepared.as_ref() {
            return Ok(backend.clone());
        }
        let backend = downloader::default_backend(cancel, reporter)?;
        *prepared = Some(backend.clone());
        Ok(backend)
    }
}
