//! The download command: listings, dry runs and the download job.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};

use crate::cancel::Cancel;
use crate::config::load_config;
use crate::downloader::{self, Deps, Tools};
use crate::error::{Error, Result};
use crate::execution::{real_wait, Adaptive};
use crate::help;
use crate::jobs::{self, JobContext};
use crate::options::Options;
use crate::output::Out;
use crate::paths;
use crate::playlist::{describe_estimate, Estimate};
use crate::progress::{Reporter, Root, TerminalReporter};
use crate::terminal::{self, path_rows, Std};
use crate::utils::{clean_text, cookie_file_warning, readable_error, validate_url};
use crate::version::CURRENT;

use super::*;

pub(super) fn run_download(args: Vec<String>, stdout: &Out, stderr: &Out) -> i32 {
    let reporter = TerminalReporter::new();
    let root: Arc<dyn Reporter> = Arc::new(Root(reporter.clone()));
    let cancel = Cancel::global();
    let mut registration = None;
    let result = (|| -> Result<i32> {
        let loaded = load_config(None)?;
        for warning in &loaded.warnings {
            stderr.write(&format!("veo: {warning}\n"));
        }
        let mut args = args;
        if args.is_empty() && terminal::stdin_is_tty() && Std::Err.is_tty() {
            match crate::interactive::interactive_args(&loaded.config, cancel)? {
                Some(answers) => args = answers,
                None => return Ok(0),
            }
        }
        let mut options = parse_cli(&args, &loaded.config)?;
        if options.help {
            stdout.write(&help::build(options.help_all));
            return Ok(0);
        }
        if options.version {
            stdout.write(&format!("{CURRENT}\n"));
            return Ok(0);
        }
        if !options.incognito {
            registration = Some(crate::runs::register_run(cancel.clone(), &paths::cache_base())?);
        }
        let mut retry_items: Option<Vec<Options>> = None;
        if let Some(retry) = options.retry_failed.clone() {
            let items = jobs::retry_options(&retry, &paths::cache_base())?;
            // Revalidate stored options and apply only flags explicitly supplied now.
            let mut retry_args = Vec::new();
            let mut skip = false;
            for (index, arg) in args.iter().enumerate() {
                if skip {
                    skip = false;
                    continue;
                }
                if arg == "--retry-failed" {
                    skip = index + 1 < args.len();
                    continue;
                }
                if arg.starts_with("--retry-failed=") {
                    continue;
                }
                retry_args.push(arg.clone());
            }
            let override_profile = retry_args.iter().any(|arg| arg == "--profile" || arg.starts_with("--profile="));
            let mut parsed = Vec::new();
            for item in items {
                let url = item.get("url").and_then(Value::as_str).unwrap_or_default().to_string();
                let mut config = item.clone();
                if override_profile {
                    if let Some(profiles) = loaded.config.get("profiles") {
                        config.insert("profiles".into(), profiles.clone());
                    }
                }
                let mut resumed = parse_cli(&[url].into_iter().chain(retry_args.iter().cloned()).collect::<Vec<_>>(), &config)?;
                if !override_profile {
                    resumed.profile = item.get("profile").and_then(Value::as_str).map(str::to_string);
                }
                parsed.push(resumed);
            }
            let urls = parsed.iter().map(|item| item.url.clone()).collect();
            options = Options { urls, ..parsed[0].clone() };
            retry_items = Some(parsed);
        }
        if let Some(batch) = options.batch_file.clone() {
            let content = String::from_utf8_lossy(&std::fs::read(&batch)?).into_owned();
            for line in content.trim_start_matches('\u{feff}').lines() {
                let line = line.trim();
                if !line.is_empty() && !line.starts_with('#') {
                    options.urls.push(validate_url(line)?);
                }
            }
            options.url = options.urls.first().cloned().unwrap_or_default();
            if options.urls.is_empty() {
                return Err(Error::msg("The URL list is empty."));
            }
            if !options.neutral_filename && options.rename.as_ref().is_some_and(|rename| !rename.contains('*')) && options.urls.len() > 1 {
                return Err(Error::msg("--rename only applies to a single URL unless the name contains * for the original title."));
            }
            if options.list_formats && options.urls.len() > 1 {
                return Err(Error::msg("--list-formats accepts exactly one URL."));
            }
        }
        let requests: Vec<Options> = retry_items.clone().unwrap_or_else(|| vec![options.clone()]);
        crate::experimental::confirm_experimental_fragments(&requests)?;
        // The job file exists before the first download, so `veo runs <id>` can follow it.
        let listing = options.list_qualities || options.list_formats || options.list_sources || options.list_subs || options.dry_run;
        let job_file = if options.incognito || listing { None } else { Some(jobs::job_file_path(&paths::cache_base())) };
        if let Some(registration) = &registration {
            registration.describe(&crate::runs::RunDetails {
                urls: &options.urls,
                output: Some(paths::display(paths::resolve(&options.output))),
                audio: options.audio,
                quality: &options.quality,
                format: options.format.as_deref(),
                profile: options.profile.as_deref(),
                playlist: options.playlist,
                job: job_file.as_ref().map(paths::display),
            })?;
        }
        reporter.configure(options.color && !options.json);
        reporter.start(options.rename.as_deref().unwrap_or(""));
        if !listing {
            reporter.profile(options.profile.as_deref());
        }
        if let Some(warning) = cookie_file_warning(options.cookies.as_deref()) {
            stderr.write(&format!("veo: {warning}\n"));
        }
        let shared = SharedBackend { prepared: Mutex::new(None) };
        let backend_resolver = |cancel: &Cancel, reporter: &dyn Reporter| shared.resolve(cancel, reporter);
        let prefetched: Prefetched = Mutex::new(HashMap::new());
        let interactive = terminal::stdin_is_tty() && Std::Err.is_tty() && !options.json;
        if retry_items.is_none() && options.urls.len() == 1 && (!options.playlist || options.list_sources) {
            let page = resolve_page_source(&options, cancel, stderr, root.as_ref(), &backend_resolver, &prefetched, interactive)?;
            if let Some(discovered) = &page.discovered {
                print_sources(stdout, &options.url, discovered, options.json);
                if options.list_sources {
                    if discovered.timed_out {
                        stderr.write(&format!("veo: Source search timed out after {}; showing verified sources found so far.\n", timeout_label(&options)));
                    }
                    if discovered.sources.is_empty() {
                        stderr.write(if discovered.timed_out {
                            "veo: No sources were verified before the timeout.\n"
                        } else {
                            "veo: No downloadable sources were found on this page.\n"
                        });
                    }
                    return Ok(if !discovered.sources.is_empty() && !discovered.timed_out { 0 } else { 1 });
                }
                if page.needs_selection {
                    stderr.write("veo: Select a source with --source <number>.\n");
                    return Ok(1);
                }
            }
            if let Some(media) = page.media_url {
                options.media_url = Some(media);
                options.source = page.source.map(|source| source.to_string());
            }
        }
        if let Some(items) = retry_items.as_mut() {
            for item in items.iter_mut().filter(|item| item.source.is_some()) {
                let page = resolve_page_source(item, cancel, stderr, root.as_ref(), &backend_resolver, &prefetched, false)?;
                item.media_url = page.media_url;
            }
        }
        let tools = Tools { cancel, reporter: root.as_ref(), backend: &backend_resolver, runner: &downloader::run_backend };
        let cached = |options: &Options| prefetched.lock().unwrap().remove(&prefetch_key(options));
        if options.list_subs {
            let result = downloader::list_subtitles(&options, &tools, cached(&options))?;
            if options.json {
                stdout.line(result.to_string());
            } else {
                stdout.line(format!("Subtitles: {}", clean_text(result["title"].as_str().unwrap_or(&options.url))));
                for (label, key) in [("Manual", "manual"), ("Automatic", "automatic")] {
                    stdout.line(format!("{label}:"));
                    let entries = result[key].as_array().cloned().unwrap_or_default();
                    if entries.is_empty() {
                        stdout.line("  none");
                    }
                    for entry in entries {
                        let name = entry["name"].as_str().map(|name| format!(" ({})", clean_text(name))).unwrap_or_default();
                        let formats: Vec<&str> = entry["formats"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                        stdout.line(format!(
                            "  {}{name}: {}",
                            clean_text(entry["language"].as_str().unwrap_or("")),
                            if formats.is_empty() { "unknown".into() } else { formats.join(", ") }
                        ));
                    }
                }
            }
            return Ok(0);
        }
        if options.list_qualities {
            let result = downloader::list_qualities(&options, &tools, cached(&options))?;
            if options.json {
                stdout.line(result.to_string());
            } else {
                let qualities: Vec<&str> = result["qualities"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                stdout.line(format!("Available qualities: {}", if qualities.is_empty() { "resolution unknown".into() } else { qualities.join(", ") }));
                if let Some(first) = qualities.first() {
                    stdout.line(format!("Choose with --quality {first}"));
                }
            }
            return Ok(0);
        }
        if options.list_formats {
            stdout.write(&downloader::list_formats(&options, &tools)?);
            return Ok(0);
        }
        if options.dry_run {
            for request in retry_items.clone().unwrap_or_else(|| options.urls.iter().map(|url| options.with_url(url)).collect()) {
                let prefetched = cached(&request);
                let (plan, estimate) = downloader::plan_download(&request, &tools, prefetched)?;
                if options.json {
                    let mut line = Map::new();
                    line.insert("url".into(), Value::String(request.url.clone()));
                    line.insert("status".into(), Value::String("planned".into()));
                    for (key, value) in plan.clone() {
                        line.insert(key, value);
                    }
                    stdout.line(Value::Object(line).to_string());
                } else {
                    print_plan(stdout, &request.url, &plan, &estimate);
                }
            }
            return Ok(0);
        }
        if !options.json && Std::Out.is_tty() {
            reporter.enable_inline();
        }
        let live: Arc<dyn crate::output::LiveRegion> = reporter.clone();
        let job_stdout = stdout.clone().with_live(live.clone());
        let job_stderr = stderr.clone().with_live(live);
        let local_root = crate::download_cache::root();
        let download = |request: &Options,
                        item_reporter: Arc<dyn Reporter>,
                        adaptive: &Adaptive,
                        on_entry: &(dyn Fn(&downloader::EntryOutcome) -> Result<()> + Sync),
                        private: Option<&Path>| {
            let deps = Deps {
                cancel,
                reporter: item_reporter,
                backend: &backend_resolver,
                runner: &downloader::run_backend,
                compatibility_runner: &downloader::run_backend,
                local_root: private.map_or_else(|| crate::download_cache::root_for(request, &local_root), Path::to_path_buf),
                skip_cache_cleanup: request.staging_dir.is_none(),
                adaptive,
                wait: &real_wait,
                disk_checker: &downloader::default_disk_checker,
                verifier: &downloader::default_verifier,
                on_entry: Some(on_entry),
            };
            downloader::download_with(request, &deps, cached(request))
        };
        jobs::run_job(
            &options,
            JobContext {
                download: &download,
                reporter: root.clone(),
                stdout: job_stdout,
                stderr: job_stderr,
                cancel,
                open_file: &crate::open_file::open_file,
                items: retry_items,
                job_file,
                run_id: registration.as_ref().map(|registration| registration.id.clone()),
                stats: (!options.incognito).then(|| crate::stats::StatsRecorder::new(&paths::cache_base())),
                history: (!options.incognito).then(|| crate::history::HistoryRecorder::new(&paths::cache_base())),
                archive_root: paths::cache_base(),
            },
        )
    })();
    let code = match result {
        Ok(code) => code,
        Err(error) => {
            let cancelled = cancel.is_cancelled() || error.is_cancelled();
            reporter.fail(cancelled);
            reporter.dispose();
            stderr.write(&format!("veo: {}\n", readable_error(&error)));
            if cancelled {
                130
            } else {
                1
            }
        }
    };
    reporter.dispose();
    if let Some(registration) = registration {
        registration.unregister();
    }
    code
}

pub(super) fn print_plan(stdout: &Out, url: &str, plan: &Map<String, Value>, estimate: &Estimate) {
    stdout.line(format!("URL: {url}"));
    stdout.line(describe_estimate(estimate));
    if let Some(quality) = plan.get("quality").and_then(Value::as_str) {
        stdout.line(quality);
    }
    for entry in plan.get("entries").and_then(Value::as_array).into_iter().flatten() {
        let target = entry["path"].as_str().unwrap_or("");
        let rows = path_rows(stdout.tty, "Would save: ", target);
        stdout.line(
            rows.iter()
                .enumerate()
                .map(|(line, row)| format!("{}{row}", if line > 0 { " ".repeat(12) } else { "Would save: ".into() }))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
}
