use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use tungstenite::{Message, WebSocket};

use crate::cancel::Cancel;
use crate::downloader::number;
use crate::error::{Error, Result};
use crate::metadata::Metadata;
use crate::paths;
use crate::utils::{clean_text, validate_url};

const MEDIA_URL: &str = r"(?i)\.(?:m3u8|mpd|mp4|webm|mov)(?:[?#]|$)";
const MEDIA_TYPE: &str = r"(?i)(?:mpegurl|dash\+xml|video/(?:mp4|webm|quicktime))";
const PLAY_CONTROL: &str = r#"(() => {
  const visited = new WeakSet();
  const state = globalThis[Symbol.for('veo.playControls')] ||= { visited, clicks: 0 };
  const labelMatches = node => {
    const label = [node.getAttribute('aria-label'), node.getAttribute('title'), node.getAttribute('data-testid'),
      node.getAttribute('value'), node.textContent].find(value => value && value.trim())?.trim() || '';
    return /^(?:play|watch|start|replay|abspielen|ansehen|wiedergabe)(?:\s+(?:video|movie|film|now))?$/i.test(label)
      || /(?:^|[-_\s])(?:big[-_])?play(?:[-_\s]|$)/i.test(node.className?.baseVal || node.className || '');
  };
  const documents = [document];
  for (let index = 0; index < documents.length && index < 16; index++) {
    for (const frame of documents[index].querySelectorAll('iframe')) {
      try { if (frame.contentDocument && !documents.includes(frame.contentDocument)) documents.push(frame.contentDocument); } catch {}
    }
  }
  for (const doc of documents) {
    const controls = doc.querySelectorAll('button,[role="button"],[aria-label],[title],[data-testid],input[type="button"],input[type="image"],[class*="play"],[class*="Play"]');
    for (const node of controls) {
      if (state.clicks >= 8) return false;
      if (state.visited.has(node) || node.disabled || node.getAttribute('aria-disabled') === 'true' || !labelMatches(node)) continue;
      const style = doc.defaultView.getComputedStyle(node);
      if (style.display === 'none' || style.visibility === 'hidden' || style.pointerEvents === 'none' || !node.getClientRects().length) continue;
      state.visited.add(node);
      state.clicks++;
      node.click();
      return true;
    }
    for (const video of doc.querySelectorAll('video')) {
      if (state.visited.has(video) || !video.paused) continue;
      state.visited.add(video);
      video.play().catch(() => {});
      return true;
    }
  }
  return false;
})()"#;

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

/// A minimal Chrome DevTools Protocol client over a blocking WebSocket.
struct Cdp {
    socket: WebSocket<TcpStream>,
    next: u64,
    replies: HashMap<u64, std::result::Result<Value, String>>,
}

impl Cdp {
    fn send(&mut self, method: &str, params: Value, session: Option<&str>) -> Result<u64> {
        self.next += 1;
        let mut message = json!({ "id": self.next, "method": method, "params": params });
        if let Some(session) = session {
            message["sessionId"] = Value::String(session.into());
        }
        self.socket.send(Message::text(message.to_string())).map_err(|error| Error::msg(format!("Browser connection failed: {error}")))?;
        Ok(self.next)
    }

    /// Read one message (or time out after the socket's read timeout).
    fn pump(&mut self, events: &mut Vec<Value>) -> Result<()> {
        match self.socket.read() {
            Ok(Message::Text(text)) => {
                let Ok(message) = serde_json::from_str::<Value>(text.as_str()) else { return Ok(()) };
                if let Some(id) = message.get("id").and_then(Value::as_u64) {
                    let reply = match message.get("error") {
                        Some(error) => Err(error.get("message").and_then(Value::as_str).unwrap_or("CDP error").to_string()),
                        None => Ok(message.get("result").cloned().unwrap_or_else(|| json!({}))),
                    };
                    self.replies.insert(id, reply);
                } else {
                    events.push(message);
                }
                Ok(())
            }
            Ok(_) => Ok(()),
            Err(tungstenite::Error::Io(error)) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Ok(()),
            Err(error) => Err(Error::msg(format!("Browser connection closed: {error}"))),
        }
    }
}

struct Capture {
    urls: Vec<String>,
    seen: HashSet<String>,
    sessions: Vec<String>,
    responses: HashMap<String, (Option<String>, String)>,
    bodies: HashSet<u64>,
    body_count: usize,
    deep: bool,
}

impl Capture {
    fn collect(&mut self, value: Option<&str>, mime: &str) {
        let Some(value) = value else { return };
        if !re!(MEDIA_URL).is_match(value) && !re!(MEDIA_TYPE).is_match(mime) {
            return;
        }
        if let Ok(url) = validate_url(value) {
            if self.seen.insert(url.clone()) {
                self.urls.push(url);
            }
        }
    }

    fn handle(&mut self, cdp: &mut Cdp, event: &Value) -> Result<()> {
        let method = event.get("method").and_then(Value::as_str).unwrap_or("");
        let params = event.get("params").cloned().unwrap_or(Value::Null);
        let session = event.get("sessionId").and_then(Value::as_str).map(str::to_string);
        match method {
            "Target.attachedToTarget" => {
                if let Some(attached) = params.get("sessionId").and_then(Value::as_str) {
                    self.sessions.push(attached.to_string());
                    cdp.send("Network.enable", json!({}), Some(attached))?;
                }
            }
            "Target.detachedFromTarget" => {
                let detached = params.get("sessionId").and_then(Value::as_str).unwrap_or("");
                self.sessions.retain(|session| session != detached);
            }
            "Network.requestWillBeSent" => self.collect(params.pointer("/request/url").and_then(Value::as_str), ""),
            "Network.responseReceived" => {
                let mime = params.pointer("/response/mimeType").and_then(Value::as_str).unwrap_or("").to_string();
                self.collect(params.pointer("/response/url").and_then(Value::as_str), &mime);
                if re!(r"(?i)(?:json|javascript|text/plain)").is_match(&mime) {
                    let request = params.get("requestId").and_then(Value::as_str).unwrap_or("").to_string();
                    self.responses.insert(format!("{}:{request}", session.clone().unwrap_or_default()), (session, request));
                }
            }
            "Network.loadingFinished" => {
                let request = params.get("requestId").and_then(Value::as_str).unwrap_or("");
                let key = format!("{}:{request}", session.clone().unwrap_or_default());
                let size = params.get("encodedDataLength").and_then(Value::as_f64).unwrap_or(0.0);
                if size > 524_288.0 {
                    return Ok(());
                }
                let Some((owner, request)) = self.responses.remove(&key) else { return Ok(()) };
                if !self.deep && self.body_count >= 100 {
                    return Ok(());
                }
                self.body_count += 1;
                let id = cdp.send("Network.getResponseBody", json!({ "requestId": request }), owner.as_deref())?;
                self.bodies.insert(id);
            }
            _ => {}
        }
        Ok(())
    }

    /// Feed finished response bodies into the URL list.
    fn settle(&mut self, cdp: &mut Cdp) {
        let finished: Vec<u64> = self.bodies.iter().copied().filter(|id| cdp.replies.contains_key(id)).collect();
        for id in finished {
            self.bodies.remove(&id);
            if let Some(Ok(result)) = cdp.replies.remove(&id) {
                if result.get("base64Encoded") != Some(&Value::Bool(true)) {
                    for url in media_urls_from_text(result.get("body").and_then(Value::as_str).unwrap_or("")) {
                        if self.seen.insert(url.clone()) {
                            self.urls.push(url);
                        }
                    }
                }
            }
        }
    }
}

fn find_debugger_port(directory: &Path, child: &mut Child, cancel: &Cancel) -> Result<u16> {
    // Cold Chromium startup on Windows runners can exceed eight seconds.
    for _ in 0..300 {
        cancel.check()?;
        if child.try_wait()?.is_some() {
            return Err(Error::msg("The browser exited before source discovery started."));
        }
        if let Ok(text) = fs::read_to_string(directory.join("DevToolsActivePort")) {
            if let Some(port) = text.lines().next().and_then(|line| line.trim().parse().ok()) {
                return Ok(port);
            }
        }
        cancel.sleep(Duration::from_millis(100))?;
    }
    Err(Error::msg("The browser did not start its debugging connection."))
}

fn launch(directory: &Path, browsers: &[PathBuf]) -> Result<Child> {
    for executable in browsers {
        let mut command = Command::new(executable);
        command
            .args([
                "--headless=new",
                "--remote-debugging-port=0",
                &format!("--user-data-dir={}", directory.display()),
                "--no-first-run",
                "--no-default-browser-check",
                "--autoplay-policy=no-user-gesture-required",
                "about:blank",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        if let Ok(child) = command.spawn() {
            return Ok(child);
        }
    }
    Err(Error::msg("No Chromium-based browser found. Set VEO_BROWSER_PATH to Chrome, Edge or Chromium."))
}

fn connect(port: u16, cancel: &Cancel) -> Result<Cdp> {
    let mut response = crate::http::get(&format!("http://127.0.0.1:{port}/json/version"), Duration::from_secs(10), &[])?;
    let endpoint: Value = serde_json::from_str(&crate::http::text(&mut response, 1024 * 1024)?)?;
    let address = endpoint.get("webSocketDebuggerUrl").and_then(Value::as_str).ok_or_else(|| Error::msg("The browser reported no debugging endpoint."))?;
    cancel.check()?;
    let stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_nodelay(true)?;
    let (socket, _) = tungstenite::client::client(address, stream).map_err(|error| Error::msg(format!("Browser connection failed: {error}")))?;
    socket.get_ref().set_read_timeout(Some(Duration::from_millis(50)))?;
    Ok(Cdp { socket, next: 0, replies: HashMap::new() })
}

/// Wait for a command's reply while handling events.
fn call(cdp: &mut Cdp, capture: &mut Capture, id: u64, cancel: &Cancel) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        cancel.check()?;
        if let Some(reply) = cdp.replies.remove(&id) {
            return reply.map_err(Error::msg);
        }
        if Instant::now() >= deadline {
            return Err(Error::msg("The browser did not answer in time."));
        }
        pump(cdp, capture)?;
    }
}

fn pump(cdp: &mut Cdp, capture: &mut Capture) -> Result<()> {
    let mut events = Vec::new();
    cdp.pump(&mut events)?;
    for event in events {
        capture.handle(cdp, &event)?;
    }
    capture.settle(cdp);
    Ok(())
}

/// Observe media requests in a temporary browser profile; no cookies are copied.
pub fn capture_browser_media(page_url: &str, cancel: &Cancel, observe: Duration, deep: bool) -> Result<Vec<String>> {
    let directory = crate::fsutil::temp_dir(&std::env::temp_dir(), "veo-discover-")?;
    let mut child: Option<Child> = None;
    let mut capture =
        Capture { urls: Vec::new(), seen: HashSet::new(), sessions: Vec::new(), responses: HashMap::new(), bodies: HashSet::new(), body_count: 0, deep };
    let result = (|| -> Result<()> {
        let browser = child.insert(launch(&directory, &browser_candidates())?);
        let port = find_debugger_port(&directory, browser, cancel)?;
        let mut cdp = connect(port, cancel)?;
        let id = cdp.send("Target.setAutoAttach", json!({ "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true }), None)?;
        call(&mut cdp, &mut capture, id, cancel)?;
        let id = cdp.send("Target.createTarget", json!({ "url": "about:blank" }), None)?;
        let target = call(&mut cdp, &mut capture, id, cancel)?;
        let id = cdp.send("Target.attachToTarget", json!({ "targetId": target["targetId"], "flatten": true }), None)?;
        let attached = call(&mut cdp, &mut capture, id, cancel)?;
        let session = attached["sessionId"].as_str().unwrap_or("").to_string();
        if !capture.sessions.contains(&session) {
            capture.sessions.push(session.clone());
        }
        for (method, params) in [("Network.enable", json!({})), ("Page.enable", json!({})), ("Page.navigate", json!({ "url": page_url }))] {
            let id = cdp.send(method, params, Some(&session))?;
            call(&mut cdp, &mut capture, id, cancel)?;
        }
        let until = Instant::now() + observe;
        while Instant::now() < until {
            cancel.check()?;
            let ids: Vec<u64> = capture
                .sessions
                .clone()
                .iter()
                .filter_map(|session| cdp.send("Runtime.evaluate", json!({ "expression": PLAY_CONTROL, "returnByValue": true }), Some(session)).ok())
                .collect();
            for id in ids {
                let _ = call(&mut cdp, &mut capture, id, cancel);
            }
            cancel.check()?;
            if !deep && !capture.urls.is_empty() {
                break;
            }
            let pause = Instant::now() + Duration::from_millis(300).min(until.saturating_duration_since(Instant::now()));
            while Instant::now() < pause {
                cancel.check()?;
                pump(&mut cdp, &mut capture)?;
            }
        }
        let settle = Instant::now() + Duration::from_secs(1);
        while !capture.bodies.is_empty() && Instant::now() < settle && !cancel.is_cancelled() {
            pump(&mut cdp, &mut capture)?;
        }
        let _ = cdp.socket.close(None);
        Ok(())
    })();
    if let Some(mut browser) = child {
        let _ = browser.kill();
        let _ = browser.wait();
    }
    for _ in 0..5 {
        if fs::remove_dir_all(&directory).is_ok() || !directory.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    match result {
        Ok(()) => Ok(capture.urls),
        Err(error) if cancel.is_cancelled() && matches!(error.kind, crate::error::Kind::Cancelled | crate::error::Kind::Timeout) => Ok(capture.urls),
        Err(error) => Err(error),
    }
}

pub type Inspect<'a> = &'a dyn Fn(&str, &Cancel) -> Result<Metadata>;
pub type Capturer<'a> = &'a dyn Fn(&str, &Cancel, Duration, bool) -> Result<Vec<String>>;

pub struct Discovered {
    pub sources: Vec<Source>,
    pub timed_out: bool,
}

fn media_key(value: &str) -> Option<String> {
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
