//! Direct HTTP media transfers. Extraction and transformations stay with yt-dlp.
//! Only anonymous requests with known-safe headers enter this path.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use ureq::http::Response;
use ureq::{Agent, Body};

use crate::cancel::Cancel;
use crate::error::{Error, Result};
use crate::metadata::Metadata;
use crate::options::Options;
use crate::progress::{Progress, Reporter};
use crate::{http, paths, state};

const MIN_RANGE: u64 = 4 * 1024 * 1024;
const UNSUPPORTED: &str = "Native HTTP fallback:";

struct Source {
    url: String,
    ext: String,
    headers: Vec<(String, String)>,
}

fn source(options: &Options, metadata: &Metadata) -> Option<Source> {
    if !options.native_http
        || options.audio
        || options.recode
        || options.cookies.is_some()
        || options.cookies_from_browser.is_some()
        || options.subs
        || options.auto_subs
        || options.sub_langs.is_some()
        || options.embed_subs
        || options.embed_metadata
        || options.embed_thumbnail
        || options.section.is_some()
        || options.sponsorblock_remove.is_some()
        || metadata.drm()
        || metadata.live()
        || metadata.is_collection()
        || metadata.formats().len() > 1
    {
        return None;
    }
    let raw = metadata.extraction.as_deref()?;
    let candidate = if raw.get("url").and_then(Value::as_str).is_some() { raw } else { raw.get("formats")?.as_array()?.first()? };
    if raw.get("requested_formats").is_some_and(|value| !value.is_null())
        || candidate.get("fragments").is_some()
        || raw.get("cookies").is_some_and(|value| !value.is_null())
        || candidate.get("has_drm").is_some_and(|value| value != &Value::Bool(false) && !value.is_null())
    {
        return None;
    }
    let protocol = candidate.get("protocol").or_else(|| raw.get("protocol"))?.as_str()?;
    if !matches!(protocol, "http" | "https") {
        return None;
    }
    let url = candidate.get("url")?.as_str()?;
    crate::utils::validate_url(url).ok()?;
    let ext = candidate.get("ext").or_else(|| raw.get("ext"))?.as_str()?;
    if !matches!(ext, "mp4" | "mkv" | "webm" | "mov" | "m4v") || options.format.as_deref().is_some_and(|format| format != ext) {
        return None;
    }
    let mut headers = std::collections::BTreeMap::new();
    for info in [raw, candidate] {
        if let Some(map) = info.get("http_headers").and_then(Value::as_object) {
            for (name, value) in map {
                let key = name.to_ascii_lowercase();
                if !matches!(
                    key.as_str(),
                    "user-agent"
                        | "referer"
                        | "accept"
                        | "accept-language"
                        | "accept-encoding"
                        | "sec-fetch-mode"
                        | "sec-fetch-site"
                        | "sec-fetch-dest"
                        | "origin"
                        | "dnt"
                ) {
                    return None;
                }
                if key != "accept-encoding" {
                    headers.insert(key, value.as_str()?.to_string());
                }
            }
        }
    }
    if options.media_url.is_some() {
        headers.insert("referer".into(), options.url.clone());
    }
    Some(Source { url: url.into(), ext: ext.into(), headers: headers.into_iter().collect() })
}

pub(super) fn source_url(options: &Options, metadata: &Metadata) -> Option<String> {
    source(options, metadata).map(|source| source.url)
}

fn client() -> &'static Agent {
    static CLIENT: OnceLock<Agent> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_resolve(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_per_read(Some(Duration::from_secs(5)))
            .user_agent(concat!("veo/", env!("CARGO_PKG_VERSION")))
            .build()
            .into()
    })
}

fn unsupported(message: &str) -> Error {
    Error::msg(format!("{UNSUPPORTED} {message}"))
}

/// Redirects are followed explicitly so only HTTP(S) is accepted, and no
/// extractor headers are forwarded to a different origin.
fn request(source: &Source, range: Option<(u64, u64)>, validator: Option<&str>, cancel: &Cancel) -> Result<Response<Body>> {
    let origin = url::Url::parse(&source.url).map_err(|_| unsupported("invalid media URL"))?.origin();
    let mut target = source.url.clone();
    for _ in 0..=10 {
        cancel.check()?;
        let parsed = url::Url::parse(&target).map_err(|_| unsupported("invalid redirect"))?;
        crate::utils::validate_url(&target).map_err(|_| unsupported("unsupported redirect"))?;
        let mut req = client().get(&target).header("Accept-Encoding", "identity");
        if parsed.origin() == origin {
            for (name, value) in &source.headers {
                req = req.header(name, value);
            }
        }
        if let Some((start, end)) = range {
            req = req.header("Range", format!("bytes={start}-{end}"));
        }
        if let Some(validator) = validator {
            req = req.header("If-Range", validator);
        }
        let response = req.call().map_err(|error| {
            // Network library diagnostics may contain signed URLs.
            match Error::from(error) {
                error if error.kind == crate::error::Kind::Timeout => error,
                _ => Error::msg("Native HTTP connection failed (connection reset or network error)."),
            }
        })?;
        if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            let location = http::header(&response, "location").ok_or_else(|| unsupported("redirect without destination"))?;
            target = parsed.join(location).map_err(|_| unsupported("invalid redirect destination"))?.to_string();
            continue;
        }
        let status = response.status().as_u16();
        if status == 429 || (500..600).contains(&status) {
            return Err(Error::msg(format!("HTTP Error {status} during native media transfer")));
        }
        if !matches!(status, 200 | 206) {
            return Err(unsupported("server declined direct transfer"));
        }
        if http::header(&response, "content-encoding").is_some_and(|encoding| !encoding.eq_ignore_ascii_case("identity")) {
            return Err(unsupported("encoded response cannot be assembled safely"));
        }
        if http::header(&response, "content-type").is_some_and(|mime| {
            let mime = mime.to_ascii_lowercase();
            mime.starts_with("text/") || mime.contains("json") || mime.contains("xml") || mime.contains("mpegurl")
        }) {
            return Err(unsupported("server returned a document instead of media"));
        }
        return Ok(response);
    }
    Err(unsupported("too many redirects"))
}

fn content_range(response: &Response<Body>) -> Option<(u64, u64, u64)> {
    let value = http::header(response, "content-range")?.strip_prefix("bytes ")?;
    let (bounds, total) = value.split_once('/')?;
    let (start, end) = bounds.split_once('-')?;
    let (start, end, total) = (start.parse().ok()?, end.parse().ok()?, total.parse().ok()?);
    (start <= end && end < total).then_some((start, end, total))
}

fn validator(response: &Response<Body>) -> Option<String> {
    http::header(response, "etag").filter(|tag| tag.starts_with('"') && tag.ends_with('"')).map(str::to_string)
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct Resume {
    version: u32,
    source: String,
    etag: String,
    total: u64,
    ranges: Vec<(u64, u64)>,
}

struct Meter<'a> {
    total: Option<u64>,
    done: AtomicU64,
    initial: u64,
    started: Instant,
    last: Mutex<Instant>,
    reporter: &'a dyn Reporter,
}

impl Meter<'_> {
    fn advance(&self, bytes: usize, finished: bool) {
        let done = self.done.fetch_add(bytes as u64, Ordering::Relaxed) + bytes as u64;
        let mut last = self.last.lock().unwrap();
        if finished || last.elapsed() >= Duration::from_millis(200) {
            let speed = (done.saturating_sub(self.initial)) as f64 / self.started.elapsed().as_secs_f64().max(0.001);
            self.reporter.progress(&Progress {
                status: Some(if finished { "finished" } else { "downloading" }.into()),
                downloaded_bytes: Some(done as f64),
                total_bytes: self.total.map(|total| total as f64),
                speed: Some(speed),
                eta: self.total.map(|total| total.saturating_sub(done) as f64 / speed.max(1.0)),
                stream: Some("Media".into()),
                ..Progress::default()
            });
            *last = Instant::now();
        }
    }
}

fn copy_body(response: &mut Response<Body>, file: &mut File, expected: Option<u64>, cancel: &Cancel, stop: &AtomicBool, meter: &Meter) -> Result<()> {
    let mut reader = response.body_mut().with_config().limit(u64::MAX).reader();
    let mut buffer = vec![0; 256 * 1024];
    let mut copied = 0;
    loop {
        cancel.check()?;
        if stop.load(Ordering::Relaxed) {
            return Err(unsupported("another range failed"));
        }
        let read = reader.read(&mut buffer).map_err(|_| Error::msg("Native HTTP connection interrupted while receiving media."))?;
        if read == 0 {
            break;
        }
        if expected.is_some_and(|expected| copied + read as u64 > expected) {
            return Err(unsupported("response exceeds the requested range"));
        }
        file.write_all(&buffer[..read])?;
        copied += read as u64;
        meter.advance(read, false);
    }
    if expected.is_some_and(|expected| copied != expected) {
        return Err(Error::msg("Native HTTP connection interrupted: incomplete response."));
    }
    file.sync_all()?;
    Ok(())
}

fn plain(source: &Source, staging: &Path, cancel: &Cancel, reporter: &dyn Reporter, response: Option<Response<Body>>) -> Result<PathBuf> {
    let mut response = match response {
        Some(response) => response,
        None => request(source, None, None, cancel)?,
    };
    if response.status().as_u16() != 200 {
        return Err(unsupported("unexpected partial response"));
    }
    let total = http::header(&response, "content-length").and_then(|value| value.parse::<u64>().ok());
    let partial = staging.join("native-full.part");
    crate::fsutil::remove_file(&partial)?;
    let mut file = state::create_private(&partial)?;
    let meter = Meter { total, done: AtomicU64::new(0), initial: 0, started: Instant::now(), last: Mutex::new(Instant::now()), reporter };
    copy_body(&mut response, &mut file, total, cancel, &AtomicBool::new(false), &meter)?;
    if file.metadata()?.len() == 0 {
        return Err(unsupported("empty media response"));
    }
    drop(file);
    let target = staging.join(format!("media.{}", source.ext));
    crate::fsutil::remove_file(&target)?;
    fs::rename(&partial, &target)?;
    meter.advance(0, true);
    Ok(target)
}

fn ranges(total: u64, connections: u32) -> Vec<(u64, u64)> {
    let count = u64::from(connections.clamp(1, 16)).min((total / MIN_RANGE).max(1));
    let size = total / count;
    (0..count).map(|index| (index * size, if index + 1 == count { total - 1 } else { (index + 1) * size - 1 })).collect()
}

fn part(staging: &Path, index: usize) -> PathBuf {
    staging.join(format!("native-{index}.part"))
}

fn discard_ranges(staging: &Path) -> Result<()> {
    for index in 0..16 {
        crate::fsutil::remove_file(&part(staging, index))?;
    }
    crate::fsutil::remove_file(&staging.join("native-data.part"))?;
    crate::fsutil::remove_file(&staging.join("native-resume.json"))
}

fn transfer(source: &Source, options: &Options, staging: &Path, cancel: &Cancel, reporter: &dyn Reporter, connections: u32) -> Result<PathBuf> {
    let mut probe = request(source, Some((0, 0)), None, cancel)?;
    if probe.status().as_u16() == 200 {
        discard_ranges(staging)?;
        return plain(source, staging, cancel, reporter, Some(probe));
    }
    let total = match content_range(&probe) {
        Some((0, 0, total)) => total,
        _ => return Err(unsupported("invalid range probe")),
    };
    let Some(etag) = validator(&probe) else {
        drop(probe);
        discard_ranges(staging)?;
        return plain(source, staging, cancel, reporter, None);
    };
    let mut byte = Vec::new();
    probe.body_mut().with_config().limit(2).reader().read_to_end(&mut byte)?;
    if byte.len() != 1 {
        return Err(unsupported("invalid range probe length"));
    }
    drop(probe);
    let marker = staging.join("native-resume.json");
    let key = state::digest(&json!(source.url));
    let previous: Option<Resume> = if options.resume { state::read_json(&marker).ok().flatten() } else { None };
    let data = staging.join("native-data.part");
    let reusable = previous.filter(|previous| {
        previous.version == 2
            && previous.source == key
            && previous.etag == etag
            && previous.total == total
            && valid_ranges(&previous.ranges, total)
            && fs::symlink_metadata(&data).is_ok_and(|info| info.is_file() && !info.file_type().is_symlink() && info.len() == total)
    });
    let layout = reusable.as_ref().map_or_else(|| ranges(total, connections), |previous| previous.ranges.clone());
    if reusable.is_none() {
        discard_ranges(staging)?;
        let file = state::create_private(&data)?;
        file.set_len(total)?;
        file.sync_all()?;
    }
    state::write_json(&marker, &Resume { version: 2, source: key, etag: etag.clone(), total, ranges: layout.clone() })?;
    let mut initial = 0;
    let mut offsets = Vec::new();
    for (index, (start, end)) in layout.iter().enumerate() {
        let file = part(staging, index);
        if fs::symlink_metadata(&file).is_ok_and(|info| !info.is_file() || info.file_type().is_symlink()) {
            return Err(unsupported("invalid partial checkpoint"));
        }
        let offset: u64 = state::read_json(&file).ok().flatten().unwrap_or(0);
        if offset > end - start + 1 {
            return Err(unsupported("invalid partial range size"));
        }
        initial += offset;
        offsets.push(offset);
    }
    let meter = Meter { total: Some(total), done: AtomicU64::new(initial), initial, started: Instant::now(), last: Mutex::new(Instant::now()), reporter };
    let next = std::sync::atomic::AtomicUsize::new(0);
    let failure = Mutex::new(None);
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for _ in 0..layout.len().min(connections.max(1) as usize) {
            scope.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&(start, end)) = layout.get(index) else { break };
                    let result = (|| -> Result<()> {
                        cancel.check()?;
                        let offset = offsets[index];
                        if offset == end - start + 1 {
                            return Ok(());
                        }
                        let mut response = request(source, Some((start + offset, end)), Some(&etag), cancel)?;
                        if response.status().as_u16() != 206
                            || content_range(&response) != Some((start + offset, end, total))
                            || validator(&response).as_deref() != Some(etag.as_str())
                        {
                            return Err(unsupported("server changed the file or ignored its range"));
                        }
                        let mut file = fs::OpenOptions::new().write(true).open(&data)?;
                        file.seek(SeekFrom::Start(start + offset))?;
                        let copied = copy_body(&mut response, &mut file, Some(end - start + 1 - offset), cancel, &stop, &meter);
                        // Publish only bytes durably written. A force-kill before
                        // this checkpoint safely re-downloads the affected range.
                        let written = file.stream_position()? - start;
                        file.sync_all()?;
                        state::write_json(&part(staging, index), &written)?;
                        copied
                    })();
                    if let Err(error) = result {
                        failure.lock().unwrap().get_or_insert(error);
                        stop.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            });
        }
    });
    if let Some(error) = failure.into_inner().unwrap() {
        return Err(error);
    }
    cancel.check()?;
    // Ranges write directly to disjoint offsets of one file: no assembly copy.
    for (index, (start, end)) in layout.iter().enumerate() {
        let written: u64 = state::read_json(&part(staging, index))?.unwrap_or(0);
        if written != end - start + 1 {
            return Err(unsupported("incomplete range"));
        }
    }
    cancel.check()?;
    let target = staging.join(format!("media.{}", source.ext));
    crate::fsutil::remove_file(&target)?;
    fs::rename(&data, &target)?;
    discard_ranges(staging)?;
    meter.advance(0, true);
    Ok(target)
}

fn valid_ranges(ranges: &[(u64, u64)], total: u64) -> bool {
    !ranges.is_empty()
        && ranges.len() <= 16
        && total > 0
        && ranges[0].0 == 0
        && ranges.last().unwrap().1 == total - 1
        && ranges.iter().all(|(start, end)| start <= end && *end < total)
        && ranges.windows(2).all(|pair| pair[0].1.checked_add(1) == Some(pair[1].0))
}

pub(super) fn download(
    options: &Options,
    metadata: &Metadata,
    staging: &Path,
    cancel: &Cancel,
    reporter: &dyn Reporter,
    connections: u32,
) -> Result<Option<Vec<String>>> {
    let Some(source) = source(options, metadata) else { return Ok(None) };
    reporter.status("Downloading direct media with Rust HTTP…");
    match transfer(&source, options, staging, cancel, reporter, connections.min(options.http_connections.max(1))) {
        Ok(file) => Ok(Some(vec![paths::display(file)])),
        Err(error) if error.message.starts_with(UNSUPPORTED) && !cancel.is_cancelled() => {
            discard_ranges(staging)?;
            reporter.status("Direct transfer unavailable; using yt-dlp…");
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::NullReporter;
    use std::io::{BufRead, BufReader};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;

    struct Server {
        url: String,
        version: Arc<std::sync::atomic::AtomicUsize>,
        requests: Arc<Mutex<Vec<Option<(u64, u64)>>>>,
        stop: Arc<AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
        directory: PathBuf,
    }

    impl Server {
        fn new(mode: usize) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            listener.set_nonblocking(true).unwrap();
            let mode = Arc::new(std::sync::atomic::AtomicUsize::new(mode));
            let version = Arc::new(std::sync::atomic::AtomicUsize::new(1));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let (m, v, r, s) = (mode.clone(), version.clone(), requests.clone(), stop.clone());
            let handle = std::thread::spawn(move || {
                let mut workers = Vec::new();
                while !s.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let (m, v, r) = (m.clone(), v.clone(), r.clone());
                            workers.push(std::thread::spawn(move || Self::respond(stream, m, v, r)));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(2)),
                        Err(error) => panic!("{error}"),
                    }
                }
                for worker in workers {
                    worker.join().unwrap();
                }
            });
            Self {
                url: format!("http://{address}/video.mp4"),
                version,
                requests,
                stop,
                handle: Some(handle),
                directory: crate::fsutil::temp_dir(&std::env::temp_dir(), "veo-native-test-").unwrap(),
            }
        }

        fn respond(
            mut stream: TcpStream,
            mode: Arc<std::sync::atomic::AtomicUsize>,
            version: Arc<std::sync::atomic::AtomicUsize>,
            requests: Arc<Mutex<Vec<Option<(u64, u64)>>>>,
        ) {
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut requested = None;
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                    let (start, end) = value.trim().split_once('-').unwrap();
                    requested = Some((start.parse::<u64>().unwrap(), end.parse::<u64>().unwrap()));
                }
            }
            requests.lock().unwrap().push(requested);
            let current_mode = mode.load(Ordering::Relaxed);
            let version = version.load(Ordering::Relaxed);
            let total = MIN_RANGE * 3 + 19;
            let partial = requested.is_some() && current_mode != 1 && !(current_mode == 3 && requested != Some((0, 0)));
            let (start, end) = if partial { requested.unwrap() } else { (0, total - 1) };
            let mut headers = format!(
                "HTTP/1.1 {}\r\nContent-Length: {}\r\nContent-Type: video/mp4\r\nConnection: close\r\n",
                if partial { "206 Partial Content" } else { "200 OK" },
                end - start + 1
            );
            if current_mode != 4 {
                headers.push_str(&format!("ETag: \"v{version}\"\r\n"));
            }
            if partial {
                headers.push_str(&format!("Content-Range: bytes {start}-{end}/{total}\r\n"));
            }
            headers.push_str("\r\n");
            if stream.write_all(headers.as_bytes()).is_err() {
                return;
            }
            let truncate = current_mode == 2 && requested != Some((0, 0)) && mode.compare_exchange(2, 0, Ordering::SeqCst, Ordering::SeqCst).is_ok();
            let length = if truncate { (end - start).div_ceil(2) } else { end - start + 1 };
            let chunk = vec![version as u8; 64 * 1024];
            let mut left = length;
            while left > 0 {
                let count = left.min(chunk.len() as u64) as usize;
                if stream.write_all(&chunk[..count]).is_err() {
                    break;
                }
                left -= count as u64;
            }
        }

        fn source(&self) -> Source {
            Source { url: self.url.clone(), ext: "mp4".into(), headers: Vec::new() }
        }
        fn options(&self) -> Options {
            Options { native_http: true, http_connections: 4, resume: true, ..Options::default() }
        }
        fn run(&self, connections: u32) -> Result<PathBuf> {
            transfer(&self.source(), &self.options(), &self.directory, &Cancel::new(), &NullReporter, connections)
        }
        fn assert_media(&self, path: &Path, value: u8) {
            let bytes = fs::read(path).unwrap();
            assert_eq!(bytes.len() as u64, MIN_RANGE * 3 + 19);
            assert!(bytes.iter().all(|byte| *byte == value));
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            let joined = self.handle.take().unwrap().join();
            let _ = fs::remove_dir_all(&self.directory);
            if !std::thread::panicking() {
                assert!(joined.is_ok());
            }
        }
    }

    #[test]
    fn parallel_ranges_produce_exact_bytes_without_an_assembly_copy() {
        let server = Server::new(0);
        let file = server.run(4).unwrap();
        server.assert_media(&file, 1);
        assert!(server.requests.lock().unwrap().len() >= 4);
        assert!(!server.directory.join("native-resume.json").exists());
    }

    #[test]
    fn interrupted_range_resumes_only_the_missing_bytes() {
        let server = Server::new(2);
        assert!(server.run(1).is_err());
        let offset: u64 = state::read_json(&part(&server.directory, 0)).unwrap().unwrap();
        assert!(offset > 0);
        let file = server.run(1).unwrap();
        server.assert_media(&file, 1);
        assert!(server.requests.lock().unwrap().iter().flatten().any(|(start, _)| *start == offset));
    }

    #[test]
    fn changed_etag_discards_previous_bytes() {
        let server = Server::new(2);
        assert!(server.run(1).is_err());
        server.version.store(2, Ordering::Relaxed);
        let file = server.run(1).unwrap();
        server.assert_media(&file, 2);
    }

    #[test]
    fn ignored_ranges_and_missing_validators_use_a_full_stream() {
        for mode in [1, 4] {
            let server = Server::new(mode);
            let file = server.run(4).unwrap();
            server.assert_media(&file, 1);
            assert!(server.requests.lock().unwrap().len() <= 2);
        }
    }

    #[test]
    fn range_mismatch_falls_back_without_publishing_a_file() {
        let server = Server::new(3);
        let metadata = Metadata { extraction: Some(Arc::new(json!({"url":server.url,"protocol":"http","ext":"mp4"}))), ..Metadata::default() };
        assert!(download(&server.options(), &metadata, &server.directory, &Cancel::new(), &NullReporter, 4).unwrap().is_none());
        assert!(!server.directory.join("media.mp4").exists());
        assert!(!server.directory.join("native-data.part").exists());
    }

    #[test]
    fn partitions_cover_exactly_the_file() {
        for size in [1, MIN_RANGE - 1, MIN_RANGE, MIN_RANGE * 8 + 7, u64::MAX - 1] {
            for connections in [1, 4, 16] {
                assert!(valid_ranges(&ranges(size, connections), size));
            }
        }
        assert!(!valid_ranges(&[(0, 3), (3, 7)], 8));
        assert!(!valid_ranges(&[(0, 3), (5, 7)], 8));
    }

    #[test]
    fn authenticated_or_transformed_media_uses_backend() {
        let raw = json!({"url":"https://example.com/video.mp4", "protocol":"https", "ext":"mp4"});
        let metadata = Metadata { extraction: Some(std::sync::Arc::new(raw.clone())), ..Metadata::default() };
        let options = Options { native_http: true, ..Options::default() };
        assert!(source(&options, &metadata).is_some());
        assert!(source(&Options { cookies: Some("cookies.txt".into()), ..options.clone() }, &metadata).is_none());
        assert!(source(&Options { section: Some("*1-2".into()), ..options.clone() }, &metadata).is_none());
        let mut raw = raw;
        raw["http_headers"] = json!({"Authorization":"Bearer private"});
        let metadata = Metadata { extraction: Some(std::sync::Arc::new(raw)), ..Metadata::default() };
        assert!(source(&options, &metadata).is_none());
    }
}
