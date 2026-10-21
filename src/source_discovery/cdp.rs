//! Capturing media requests in a headless browser over the DevTools protocol.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tungstenite::{Message, WebSocket};

use crate::cancel::Cancel;
use crate::error::{Error, Result};
use crate::utils::validate_url;

use super::*;

pub(super) const PLAY_CONTROL: &str = r#"(() => {
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

/// A minimal Chrome DevTools Protocol client over a blocking WebSocket.
pub(super) struct Cdp {
    pub(super) socket: WebSocket<TcpStream>,
    pub(super) next: u64,
    pub(super) replies: HashMap<u64, std::result::Result<Value, String>>,
}

impl Cdp {
    pub(super) fn send(&mut self, method: &str, params: Value, session: Option<&str>) -> Result<u64> {
        self.next += 1;
        let mut message = json!({ "id": self.next, "method": method, "params": params });
        if let Some(session) = session {
            message["sessionId"] = Value::String(session.into());
        }
        self.socket.send(Message::text(message.to_string())).map_err(|error| Error::msg(format!("Browser connection failed: {error}")))?;
        Ok(self.next)
    }

    /// Read one message (or time out after the socket's read timeout).
    pub(super) fn pump(&mut self, events: &mut Vec<Value>) -> Result<()> {
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

pub(super) struct Capture {
    pub(super) urls: Vec<String>,
    pub(super) seen: HashSet<String>,
    pub(super) sessions: Vec<String>,
    pub(super) responses: HashMap<String, (Option<String>, String)>,
    pub(super) bodies: HashSet<u64>,
    pub(super) body_count: usize,
    pub(super) deep: bool,
}

impl Capture {
    pub(super) fn collect(&mut self, value: Option<&str>, mime: &str) {
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

    pub(super) fn handle(&mut self, cdp: &mut Cdp, event: &Value) -> Result<()> {
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
    pub(super) fn settle(&mut self, cdp: &mut Cdp) {
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

pub(super) fn find_debugger_port(directory: &Path, child: &mut Child, cancel: &Cancel) -> Result<u16> {
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

pub(super) fn launch(directory: &Path, browsers: &[PathBuf]) -> Result<Child> {
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

pub(super) fn connect(port: u16, cancel: &Cancel) -> Result<Cdp> {
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
pub(super) fn call(cdp: &mut Cdp, capture: &mut Capture, id: u64, cancel: &Cancel) -> Result<Value> {
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

pub(super) fn pump(cdp: &mut Cdp, capture: &mut Capture) -> Result<()> {
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
